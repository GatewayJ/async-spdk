use spdk_sys::*;
use std::mem::{ManuallyDrop, MaybeUninit};
use std::ops::{Deref, DerefMut};

/// DMA allocation with an explicit initialization state.
#[derive(Debug)]
pub struct DmaBuf<T = MaybeUninit<u8>> {
    ptr: *mut T,
    len: usize,
}

unsafe impl<T: Send> Send for DmaBuf<T> {}

impl DmaBuf<MaybeUninit<u8>> {
    pub fn alloc(size: usize, align: usize) -> Self {
        let ptr = unsafe { spdk_dma_malloc(size as _, align as _, std::ptr::null_mut()) };
        assert!(!ptr.is_null(), "Failed to allocate DMA memory");
        Self {
            ptr: ptr.cast(),
            len: size,
        }
    }

    pub fn alloc_zeroed(size: usize, align: usize) -> DmaBuf<u8> {
        let ptr = unsafe { spdk_dma_zmalloc(size as _, align as _, std::ptr::null_mut()) };
        assert!(!ptr.is_null(), "Failed to allocate DMA memory");
        DmaBuf {
            ptr: ptr.cast(),
            len: size,
        }
    }

    /// Mark every byte as initialized without copying the allocation.
    ///
    /// # Safety
    /// Every byte must have been initialized by the caller or a completed read.
    pub unsafe fn assume_init(self) -> DmaBuf<u8> {
        let this = ManuallyDrop::new(self);
        DmaBuf {
            ptr: this.ptr.cast(),
            len: this.len,
        }
    }
}

impl<T> DmaBuf<T> {
    pub fn as_ptr(&self) -> *const T {
        self.ptr
    }
}
impl<T> AsRef<[T]> for DmaBuf<T> {
    fn as_ref(&self) -> &[T] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}
impl<T> AsMut<[T]> for DmaBuf<T> {
    fn as_mut(&mut self) -> &mut [T] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}
impl<T> Deref for DmaBuf<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_ref()
    }
}
impl<T> DerefMut for DmaBuf<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut()
    }
}
impl<T> Drop for DmaBuf<T> {
    fn drop(&mut self) {
        unsafe { spdk_dma_free(self.ptr.cast()) }
    }
}

/// Return the reactor running this thread, or None outside an SPDK core.
pub fn get_current_core() -> Option<u32> {
    let core = unsafe { spdk_env_get_current_core() };
    (core != u32::MAX).then_some(core)
}

pub fn get_reactor_cores() -> Vec<u32> {
    let mut cores = Vec::new();
    let mut core = unsafe { spdk_env_get_first_core() };
    while core != u32::MAX {
        cores.push(core);
        core = unsafe { spdk_env_get_next_core(core) };
    }
    cores
}
