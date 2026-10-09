use crate::error::*;
use crate::{complete::LocalComplete, SpdkError};
use spdk_sys::*;
use std::{
    cell::RefCell,
    ffi::{c_void, CString},
    future::Future,
    mem::MaybeUninit,
    os::raw::c_int,
    pin::Pin,
    rc::Rc,
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

pub struct AppOpts(
    spdk_app_opts,
    Option<CString>,
    Option<CString>,
    Option<CString>,
    Option<CString>,
);

impl AppOpts {
    pub fn new() -> Self {
        let mut opts = MaybeUninit::uninit();
        extern "C" {
            fn spdk_app_opts_init(opts: *mut spdk_app_opts, size: usize);
        }
        unsafe {
            spdk_app_opts_init(opts.as_mut_ptr(), std::mem::size_of::<spdk_app_opts>());
            AppOpts(opts.assume_init(), None, None, None, None)
        }
    }

    pub fn get_opts(&self) -> spdk_app_opts {
        self.0
    }

    pub fn name(mut self, name: &str) -> Self {
        self.1 = Some(CString::new(name).expect("Invalid application name"));
        self.0.name = self.1.as_ref().unwrap().as_ptr();
        self
    }

    pub fn config_file(mut self, config_file: &str) -> Self {
        self.2 = Some(CString::new(config_file).expect("Invalid configuration path"));
        self.0.json_config_file = self.2.as_ref().unwrap().as_ptr();
        self
    }

    pub fn reactor_mask(mut self, mask: &str) -> Self {
        self.3 = Some(CString::new(mask).expect("Invalid reactor mask"));
        self.0.reactor_mask = self.3.as_ref().unwrap().as_ptr();
        self
    }

    /// Use ordinary memory for Malloc or AIO testing without hugepages.
    pub fn no_huge(mut self, enabled: bool) -> Self {
        if enabled {
            self.0.unlink_hugepage = false;
            self.0.hugepage_single_segments = true;
        }
        self.4 = enabled
            .then(|| CString::new("--no-huge --legacy-mem --no-shconf --iova-mode=va").unwrap());
        self.0.env_context = self
            .4
            .as_ref()
            .map_or(std::ptr::null_mut(), |s| s.as_ptr() as *mut c_void);
        self
    }

    pub fn no_pci(mut self, enabled: bool) -> Self {
        self.0.no_pci = enabled;
        self
    }

    /// Set trace entries per core; zero disables the tracing shared-memory buffer.
    pub fn trace_entries(mut self, entries: u64) -> Self {
        self.0.num_entries = entries;
        self
    }

    pub fn memory_size_mb(mut self, size: i32) -> Self {
        self.0.mem_size = size;
        self
    }

    pub fn set_log(mut self, log_level: i32) -> Self {
        self.0.print_level = log_level;
        self
    }

    pub fn main_core(mut self, main_core: i32) -> Self {
        self.0.main_core = main_core;
        self
    }

    pub fn block_on<F: Future>(mut self, future: F) -> F::Output {
        extern "C" fn start_fn<F: Future>(arg: *mut c_void) {
            register_current_reactor_thread();
            let (future, output_ptr) = unsafe { *Box::from_raw(arg as *mut (F, *mut F::Output)) };
            spawn_internal(future, output_ptr);
        }
        let mut output = MaybeUninit::uninit();
        unsafe {
            let err = spdk_app_start(
                &mut self.0,
                Some(start_fn::<F>),
                Box::into_raw(Box::new((future, output.as_mut_ptr()))) as *mut c_void,
            );
            assert_eq!(err, 0);
            reactor_threads().lock().unwrap().clear();
            spdk_app_fini();
            output.assume_init()
        }
    }
}

pub fn spawn<'a, F: Future + 'a>(future: F) -> JoinHandle<F> {
    spawn_internal(future, std::ptr::null_mut())
}

// The ownership of this `Task` is shared by `JoinHandle` and poller.
struct Task<F: Future> {
    future: F,
    poller: *mut spdk_poller,
    waker: Waker,
    /// If this pointer is non-null, then put future output here.
    output_ptr: *mut F::Output,
    output: LocalComplete<F::Output>,
}

pub struct JoinHandle<F: Future> {
    task: Rc<RefCell<Task<F>>>,
}

impl<F: Future<Output = T>, T: Unpin> Future for JoinHandle<F> {
    type Output = F::Output;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut task = self.task.borrow_mut();
        Pin::new(&mut task.output).poll(cx)
    }
}

fn spawn_internal<F: Future>(future: F, output_ptr: *mut F::Output) -> JoinHandle<F> {
    extern "C" fn poller_wrapper<F: Future>(cell_ptr: *mut c_void) -> c_int {
        let cell_ptr = cell_ptr as *const RefCell<Task<F>>;
        let task = &mut *unsafe { &*cell_ptr }.borrow_mut();
        let mut context = Context::from_waker(&task.waker);
        let future = unsafe { Pin::new_unchecked(&mut task.future) };
        match future.poll(&mut context) {
            Poll::Pending => unsafe { spdk_poller_pause(task.poller) },
            Poll::Ready(output) => unsafe {
                if task.output_ptr.is_null() {
                    task.output.complete(output);
                } else {
                    task.output_ptr.write(output);
                    // spdk_app_stop(0);
                }
                spdk_poller_unregister(&mut task.poller);
                Rc::from_raw(cell_ptr);
            },
        }
        // return positive to indicate that polling took place and some events were processed.
        1
    }
    let task = Rc::new(RefCell::new(Task {
        future,
        poller: std::ptr::null_mut(),
        waker: unsafe { poller_waker(std::ptr::null_mut()) },
        output_ptr,
        output: LocalComplete::new(),
    }));
    // move a reference to the poller
    let arg = Rc::into_raw(task.clone());
    let poller = unsafe { spdk_poller_register(Some(poller_wrapper::<F>), arg as _, 0) };
    assert!(!poller.is_null());
    {
        let mut task0 = task.borrow_mut();
        task0.poller = poller;
        task0.waker = unsafe { poller_waker(poller) };
    }
    JoinHandle { task }
}

unsafe fn poller_waker(poller: *mut spdk_poller) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        |data| RawWaker::new(data, &VTABLE),                 // clone
        |poller| unsafe { spdk_poller_resume(poller as _) }, // wake
        |poller| unsafe { spdk_poller_resume(poller as _) }, // wake_by_ref
        |_| {},                                              // drop
    );
    Waker::from_raw(RawWaker::new(poller as _, &VTABLE))
}

pub fn send_shutdown() {
    unsafe {
        spdk_app_start_shutdown();
    }
}

pub fn app_stop() {
    unsafe {
        spdk_app_stop(0);
    }
}

pub fn app_fini() {
    unsafe {
        spdk_app_fini();
    }
}

#[derive(Debug, Clone)]
pub struct SpdkEvent {
    ptr: *mut spdk_event,
}

impl SpdkEvent {
    /// allocate a event
    pub fn alloc(lcore: u32, arg1: *mut c_void, arg2: *mut c_void) -> Result<Self> {
        let ptr = unsafe { spdk_event_allocate(lcore, Some(callback2), arg1, arg2) };
        if ptr.is_null() {
            return Err(SpdkError::from(-1));
        }
        Ok(SpdkEvent { ptr })
    }

    /// put this event to specific reactor event ring
    pub fn call(&self) -> Result<()> {
        unsafe {
            spdk_event_call(self.ptr);
        }
        Ok(())
    }
}

extern "C" fn callback2(arg1: *mut c_void, arg2: *mut c_void) {
    let f: fn(*mut c_void) = unsafe { std::mem::transmute(arg1) };
    f(arg2);
}

// The options own their strings and are moved to the thread running app_start.
unsafe impl Send for AppOpts {}

impl AppOpts {
    /// Run SPDK until shutdown. Completing the initializer keeps the reactors running.
    pub fn app_start<F: Future<Output = ()> + 'static>(mut self, future: F) -> Result<()> {
        extern "C" fn initialize<F: Future<Output = ()> + 'static>(arg: *mut c_void) {
            register_current_reactor_thread();
            let future = unsafe { &mut *arg.cast::<Option<F>>() }.take().unwrap();
            spawn_local(future).detach();
        }
        let mut future = Some(future);
        let rc = unsafe {
            spdk_app_start(
                &mut self.0,
                Some(initialize::<F>),
                (&mut future as *mut Option<F>).cast(),
            )
        };
        reactor_threads().lock().unwrap().clear();
        unsafe { spdk_app_fini() };
        SpdkError::from_retval(if rc > 0 {
            -(nix::errno::Errno::EIO as i32)
        } else {
            rc
        })
    }
}

/// Spawn a future on the current SPDK thread. Wakes are sent back to that thread.
pub fn spawn_local<F>(future: F) -> async_task::Task<F::Output>
where
    F: Future + 'static,
    F::Output: 'static,
{
    let thread = unsafe { spdk_get_thread() } as usize;
    assert_ne!(thread, 0, "spawn_local requires an SPDK thread");
    extern "C" fn run(arg: *mut c_void) {
        let runnable = unsafe { Box::from_raw(arg.cast::<async_task::Runnable>()) };
        runnable.run();
    }
    let schedule = move |runnable: async_task::Runnable| {
        let arg = Box::into_raw(Box::new(runnable));
        let rc = unsafe { spdk_thread_send_msg(thread as *mut spdk_thread, Some(run), arg.cast()) };
        // A live SPDK task must retain its reactor until all completion callbacks finish.
        assert_eq!(rc, 0, "SPDK thread stopped before task completion");
    };
    let (runnable, task) = async_task::spawn_local(future, schedule);
    runnable.schedule();
    task
}

fn reactor_threads() -> &'static std::sync::Mutex<std::collections::BTreeMap<u32, usize>> {
    static THREADS: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<u32, usize>>> =
        std::sync::OnceLock::new();
    THREADS.get_or_init(Default::default)
}

fn register_current_reactor_thread() {
    let core = crate::env::get_current_core().expect("SPDK initializer must run on a reactor");
    reactor_threads()
        .lock()
        .unwrap()
        .insert(core, unsafe { spdk_get_thread() } as usize);
}

impl SpdkEvent {
    /// Run a closure on a persistent SPDK thread pinned to the selected reactor.
    pub fn call_on<F: FnOnce() + Send + 'static>(core: u32, f: F) -> Result<()> {
        if !crate::env::get_reactor_cores().contains(&core) {
            return Err(nix::errno::Errno::EINVAL.into());
        }
        let thread = {
            let mut threads = reactor_threads().lock().unwrap();
            if let Some(&thread) = threads.get(&core) {
                thread as *mut spdk_thread
            } else {
                let mut mask = MaybeUninit::<spdk_cpuset>::uninit();
                let name = CString::new(format!("async-spdk-{}", core)).unwrap();
                let thread = unsafe {
                    spdk_cpuset_zero(mask.as_mut_ptr());
                    spdk_cpuset_set_cpu(mask.as_mut_ptr(), core, true);
                    spdk_thread_create(name.as_ptr(), mask.as_ptr())
                };
                if thread.is_null() {
                    return Err(nix::errno::Errno::ENOMEM.into());
                }
                threads.insert(core, thread as usize);
                thread
            }
        };
        extern "C" fn invoke<F: FnOnce()>(arg: *mut c_void) {
            let f = unsafe { Box::from_raw(arg.cast::<F>()) };
            f();
        }
        let arg = Box::into_raw(Box::new(f));
        let rc = unsafe { spdk_thread_send_msg(thread, Some(invoke::<F>), arg.cast()) };
        if rc != 0 {
            unsafe { drop(Box::from_raw(arg)) };
            return Err(SpdkError::from(rc));
        }
        Ok(())
    }
}
