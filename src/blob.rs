//! Blob Storage System

use crate::{blob_bdev::BlobStoreBDev, error::*};
use log::*;
use serde::{Deserialize, Serialize};
use spdk_sys::*;
use std::convert::TryInto;
use std::ffi::{c_void, CString};
use std::fmt;
use std::mem::MaybeUninit;
use std::os::raw::c_int;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

#[derive(Debug)]
pub struct Blobstore {
    pub ptr: *mut spdk_blob_store,
}

impl Default for Blobstore {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
        }
    }
}

unsafe impl Send for Blobstore {}
unsafe impl Sync for Blobstore {}

impl Blobstore {
    /// Get the cluster size in bytes.
    pub fn cluster_size(&self) -> u64 {
        unsafe { spdk_bs_get_cluster_size(self.ptr) }
    }

    /// Get the page size in bytes.
    pub fn page_size(&self) -> u64 {
        unsafe { spdk_bs_get_page_size(self.ptr) }
    }

    /// Get the io unit size in bytes.
    pub fn io_unit_size(&self) -> u64 {
        unsafe { spdk_bs_get_io_unit_size(self.ptr) }
    }

    /// Get the number of free clusters.
    pub fn free_cluster_count(&self) -> u64 {
        unsafe { spdk_bs_free_cluster_count(self.ptr) }
    }

    /// Get the total number of clusters accessible by user.
    pub fn total_data_cluster_count(&self) -> u64 {
        unsafe { spdk_bs_total_data_cluster_count(self.ptr) }
    }

    /// Allocate an I/O channel for the given blobstore.
    pub fn alloc_io_channel(&self) -> Result<IoChannel> {
        let ptr = unsafe { spdk_bs_alloc_io_channel(self.ptr) };
        if ptr.is_null() {
            // FIXME: proper error
            return Err(SpdkError::from(-1));
        }
        Ok(IoChannel { ptr })
    }

    /// Initialize a blobstore on the given device.
    pub async fn init(bs_dev: &mut BlobStoreBDev) -> Result<Blobstore> {
        let ptr = do_async(|arg| unsafe {
            spdk_bs_init(bs_dev.ptr, std::ptr::null_mut(), Some(callback_with), arg);
        })
        .await?;
        Ok(Blobstore { ptr })
    }

    pub fn init_sync(bs_dev: &mut BlobStoreBDev, cb_arg: *mut c_void) -> Result<()> {
        unsafe {
            spdk_bs_init(
                bs_dev.ptr,
                std::ptr::null_mut(),
                Some(init_callback),
                cb_arg,
            );
        };
        Ok(())
    }

    /// Load a blobstore on the given device
    pub async fn load(bs_dev: &mut BlobStoreBDev) -> Result<Blobstore> {
        let ptr = do_async(|arg| unsafe {
            spdk_bs_load(bs_dev.ptr, std::ptr::null_mut(), Some(callback_with), arg);
        })
        .await?;
        Ok(Blobstore { ptr })
    }

    pub fn load_sync(bs_dev: &mut BlobStoreBDev, cb_arg: *mut c_void) -> Result<()> {
        unsafe {
            spdk_bs_load(
                bs_dev.ptr,
                std::ptr::null_mut(),
                Some(init_callback),
                cb_arg,
            );
        };
        Ok(())
    }

    /// Unload the blobstore.
    ///
    /// It will flush all volatile data to disk.
    /// WARN: all io_channels must be dropped before unload!
    pub async fn unload(&self) -> Result<()> {
        do_async(|arg| unsafe {
            spdk_bs_unload(self.ptr, Some(callback), arg);
        })
        .await?;
        Ok(())
    }

    pub fn unload_sync(&self, cb_arg: *mut c_void) -> Result<()> {
        if self.ptr.is_null() {
            error!("blobstore ptr is null");
        }
        unsafe {
            spdk_bs_unload(self.ptr, Some(unload_callback), cb_arg);
        }
        Ok(())
    }

    /// Create a new blob with default option values on the given blobstore.
    pub async fn create_blob(&self) -> Result<BlobId> {
        let id = do_async(|arg| unsafe {
            spdk_bs_create_blob(self.ptr, Some(callback_with), arg);
        })
        .await?;
        Ok(BlobId { id })
    }

    /// Create blob, sync API
    ///
    /// cb_arg: Arc<Mutex< BlobId >>
    pub fn create_blob_sync(
        &self,
        // cb_fn: extern "C" fn(*mut c_void, spdk_blob_id, i32),
        cb_arg: *mut c_void,
    ) -> Result<()> {
        unsafe {
            spdk_bs_create_blob(self.ptr, Some(create_callback), cb_arg);
        }
        Ok(())
    }

    /// Open a blob from the given blobstore.
    pub async fn open_blob(&self, blob_id: BlobId) -> Result<Blob> {
        let ptr = do_async(|arg| unsafe {
            spdk_bs_open_blob(self.ptr, blob_id.id, Some(callback_with), arg);
        })
        .await?;
        let io_unit_size = self.io_unit_size();
        Ok(Blob { ptr, io_unit_size })
    }

    /// Open blob, sync API
    ///
    /// cb_arg: Arc<Mutex<Blob>>
    pub fn open_blob_sync(
        &self,
        blob_id: &BlobId,
        // cb_fn: extern "C" fn(*mut c_void, *mut spdk_blob, c_int),
        cb_arg: *mut c_void,
    ) -> Result<()> {
        unsafe {
            spdk_bs_open_blob(self.ptr, blob_id.id, Some(open_callback), cb_arg);
        }
        Ok(())
    }

    /// Delete an existing blob from the given blobstore.
    pub async fn delete_blob(&self, blob_id: BlobId) -> Result<()> {
        do_async(|arg| unsafe {
            spdk_bs_delete_blob(self.ptr, blob_id.id, Some(callback), arg);
        })
        .await?;
        Ok(())
    }

    /// Delete blob, sync API
    pub fn delete_blob_sync(&self, blob_id: &BlobId, cb_arg: *mut c_void) -> Result<()> {
        unsafe {
            spdk_bs_delete_blob(self.ptr, blob_id.id, Some(delete_callback), cb_arg);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BlobId {
    id: spdk_blob_id,
}

impl Default for BlobId {
    fn default() -> Self {
        Self { id: 0 }
    }
}

impl fmt::Display for BlobId {
    fn fmt<'a>(&self, f: &mut fmt::Formatter<'a>) -> fmt::Result {
        write!(f, "BlobId({:?})", self.id)
    }
}

#[derive(Debug)]
pub struct IoChannel {
    pub ptr: *mut spdk_io_channel,
}

unsafe impl Send for IoChannel {}
unsafe impl Sync for IoChannel {}

impl Drop for IoChannel {
    fn drop(&mut self) {
        unsafe { spdk_bs_free_io_channel(self.ptr) };
    }
}

#[derive(Debug)]
pub struct Blob {
    pub ptr: *mut spdk_blob,
    io_unit_size: u64,
}

impl Default for Blob {
    fn default() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            io_unit_size: 512,
        }
    }
}

unsafe impl Send for Blob {}
unsafe impl Sync for Blob {}

impl Blob {
    /// Get the number of clusters allocated to the blob.
    pub fn num_clusters(&self) -> u64 {
        unsafe { spdk_blob_get_num_clusters(self.ptr) }
    }

    /// Get the blob id.
    pub fn blob_id(&self) -> BlobId {
        let id = unsafe { spdk_blob_get_id(self.ptr) };
        BlobId { id }
    }

    /// Read data from a blob.
    pub async fn read(
        &self,
        io_channel: &IoChannel,
        offset: u64,
        buf: &mut [MaybeUninit<u8>],
    ) -> Result<()> {
        let (offset, units) = io_range(offset, buf.len() as u64, self.io_unit_size)?;
        do_async(|arg| unsafe {
            spdk_blob_io_read(
                self.ptr,
                io_channel.ptr,
                buf.as_mut_ptr() as _,
                offset,
                units,
                Some(callback),
                arg,
            );
        })
        .await
    }

    /// Read data from a blob, sync API
    pub fn read_sync(
        &self,
        io_channel: &IoChannel,
        offset: u64,
        buf: &mut [u8],
        cb_arg: *mut c_void,
    ) -> Result<()> {
        assert_eq!(buf.len() as u64 % self.io_unit_size, 0);
        let units = buf.len() as u64 / self.io_unit_size;
        unsafe {
            spdk_blob_io_read(
                self.ptr,
                io_channel.ptr,
                buf.as_mut_ptr() as _,
                offset,
                units,
                Some(rw_callback),
                cb_arg,
            );
        }
        Ok(())
    }

    /// Write data to a blob.
    pub async fn write(&self, io_channel: &IoChannel, offset: u64, buf: &[u8]) -> Result<()> {
        let (offset, units) = io_range(offset, buf.len() as u64, self.io_unit_size)?;
        do_async(|arg| unsafe {
            spdk_blob_io_write(
                self.ptr,
                io_channel.ptr,
                buf.as_ptr() as _,
                offset,
                units,
                Some(callback),
                arg,
            );
        })
        .await
    }

    /// Write data to a blob, sync API
    pub fn write_sync(
        &self,
        io_channel: &IoChannel,
        offset: u64,
        buf: &[u8],
        cb_arg: *mut c_void,
    ) -> Result<()> {
        assert_eq!(buf.len() as u64 % self.io_unit_size, 0);
        let units = buf.len() as u64 / self.io_unit_size;
        unsafe {
            spdk_blob_io_write(
                self.ptr,
                io_channel.ptr,
                buf.as_ptr() as _,
                offset,
                units,
                Some(rw_callback),
                cb_arg,
            );
        }
        Ok(())
    }

    /// Write zeros into area of a blob.
    pub async fn write_zero(&self, io_channel: &IoChannel, offset: u64, len: u64) -> Result<()> {
        let (offset, units) = io_range(offset, len, self.io_unit_size)?;
        do_async(|arg| unsafe {
            spdk_blob_io_write_zeroes(self.ptr, io_channel.ptr, offset, units, Some(callback), arg);
        })
        .await
    }

    /// Write zeros to a blob, sync API
    pub fn write_zero_sync(
        &self,
        io_channel: &IoChannel,
        offset: u64,
        len: u64,
        cb_arg: *mut c_void,
    ) -> Result<()> {
        assert_eq!(len % self.io_unit_size, 0);
        let units = len / self.io_unit_size;
        unsafe {
            spdk_blob_io_write_zeroes(
                self.ptr,
                io_channel.ptr,
                offset,
                units,
                Some(rw_callback),
                cb_arg,
            );
        }
        Ok(())
    }

    /// Resize a blob to `size` clusters.
    ///
    /// These changes are not persisted to disk until spdk_bs_md_sync_blob() is called.
    /// If called before previous resize finish, it will fail with errno -EBUSY.
    pub async fn resize(&self, size: u64) -> Result<()> {
        do_async(|arg| unsafe {
            spdk_blob_resize(self.ptr, size, Some(callback), arg);
        })
        .await?;
        Ok(())
    }

    /// Resize a blob, sync API
    pub fn resize_sync(
        &self,
        size: u64,
        // cb_fn: unsafe extern "C" fn(*mut c_void, c_int),
        cb_arg: *mut c_void,
    ) -> Result<()> {
        unsafe {
            spdk_blob_resize(self.ptr, size, Some(resize_callback), cb_arg);
        }
        Ok(())
    }

    /// Sync a blob.
    ///
    /// Make a blob persistent. This applies to open, resize, set xattr, and remove xattr.
    /// These operations will not be persistent until the blob has been synced.
    pub async fn sync_metadata(&self) -> Result<()> {
        do_async(|arg| unsafe {
            spdk_blob_sync_md(self.ptr, Some(callback), arg);
        })
        .await?;
        Ok(())
    }

    /// Sync blob's metadata, sync API
    pub fn sync_metadata_sync(
        &self,
        // cb_fn: unsafe extern "C" fn(*mut c_void, c_int),
        cb_arg: *mut c_void,
    ) -> Result<()> {
        unsafe {
            spdk_blob_sync_md(self.ptr, Some(sync_md_callback), cb_arg);
        }
        Ok(())
    }

    /// Close a blob.
    ///
    /// This will automatically sync.
    pub async fn close(mut self) -> Result<()> {
        let ptr = std::mem::replace(&mut self.ptr, std::ptr::null_mut());
        do_async(|arg| unsafe {
            spdk_blob_close(ptr, Some(callback), arg);
        })
        .await?;
        Ok(())
    }

    /// Close a blob, sync API
    pub fn close_sync(mut self, cb_arg: *mut c_void) -> Result<()> {
        let ptr = std::mem::replace(&mut self.ptr, std::ptr::null_mut());
        unsafe {
            spdk_blob_close(ptr, Some(close_blob_callback), cb_arg);
        }
        Ok(())
    }
}

extern "C" fn callback(arg: *mut c_void, bserrno: c_int) {
    callback_with(arg, (), bserrno);
}

extern "C" fn callback_with<T>(arg: *mut c_void, bs: T, bserrno: c_int) {
    let complete =
        unsafe { Box::from_raw(arg.cast::<futures::channel::oneshot::Sender<Result<T>>>()) };
    let result = if bserrno != 0 {
        Err(SpdkError::from(bserrno))
    } else {
        Ok(bs)
    };
    let _ = complete.send(result);
}

extern "C" fn init_callback(mut arg: *mut c_void, bs: *mut spdk_blob_store, bserrno: c_int) {
    if bserrno != 0 {
        error!("bs error");
    }
    if bs.is_null() {
        error!("bs pointer is null");
    }
    let (bs_, n) = unsafe { *Box::from_raw(arg as *mut (Arc<Mutex<Blobstore>>, Arc<Notify>)) };
    unsafe {
        bs_.lock().unwrap().ptr = bs;
        n.notify_one();
    }
}

extern "C" fn open_callback(mut arg: *mut c_void, blob: *mut spdk_blob, bserrno: c_int) {
    if bserrno != 0 {
        error!("open error");
    }
    if blob.is_null() {
        error!("open blob pointer null");
    }
    let (blob_, n) = unsafe { *Box::from_raw(arg as *mut (Arc<Mutex<Blob>>, Arc<Notify>)) };
    unsafe {
        blob_.lock().unwrap().ptr = blob;
        n.notify_one();
    }
}

extern "C" fn create_callback(mut arg: *mut c_void, blob_id: spdk_blob_id, bserrno: c_int) {
    if bserrno != 0 {
        error!("create error");
    }
    let (blob_id_, n) = unsafe { *Box::from_raw(arg as *mut (Arc<Mutex<BlobId>>, Arc<Notify>)) };
    unsafe {
        blob_id_.lock().unwrap().id = blob_id;
        n.notify_one();
    }
}

extern "C" fn resize_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("resize error");
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

extern "C" fn sync_md_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("sync metadata error");
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

extern "C" fn unload_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("bs unload error");
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

extern "C" fn rw_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("read/write error: {}", bserrno);
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

extern "C" fn delete_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("delete blob error");
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

extern "C" fn close_blob_callback(arg: *mut c_void, bserrno: c_int) {
    if bserrno != 0 {
        error!("close blob error");
    }
    let n = unsafe { *Box::from_raw(arg as *mut Arc<Notify>) };
    n.notify_one();
}

async fn do_async<T>(f: impl FnOnce(*mut c_void)) -> Result<T> {
    let (tx, rx) = futures::channel::oneshot::channel::<Result<T>>();
    f(Box::into_raw(Box::new(tx)).cast());
    rx.await.expect("SPDK completion callback dropped")
}

/// Convert byte offsets and lengths to SPDK I/O units.
fn io_range(offset: u64, length: u64, unit: u64) -> Result<(u64, u64)> {
    if unit == 0 || offset % unit != 0 || length % unit != 0 || offset.checked_add(length).is_none()
    {
        return Err(nix::errno::Errno::EINVAL.into());
    }
    Ok((offset / unit, length / unit))
}

pub type BlobStore = Blobstore;

impl Blobstore {
    pub fn get_io_channel(&self) -> IoChannel {
        self.alloc_io_channel()
            .expect("Failed to allocate blob I/O channel")
    }

    pub async fn get_super(&self) -> Result<BlobId> {
        let id = do_async(|arg| unsafe {
            spdk_bs_get_super(self.ptr, Some(callback_with), arg);
        })
        .await?;
        Ok(BlobId { id })
    }

    pub async fn set_super(&self, blob_id: BlobId) -> Result<()> {
        do_async(|arg| unsafe {
            spdk_bs_set_super(self.ptr, blob_id.id, Some(callback), arg);
        })
        .await
    }

    /// Each yielded blob owns an open reference independent of the iterator.
    pub fn list_blobs(&mut self) -> BlobIterator<'_> {
        BlobIterator {
            store: self,
            current: std::ptr::null_mut(),
            finished: false,
        }
    }
}

pub struct BlobIterator<'a> {
    store: &'a Blobstore,
    current: *mut spdk_blob,
    finished: bool,
}

impl BlobIterator<'_> {
    pub async fn next(&mut self) -> Option<Result<Blob>> {
        if self.finished {
            return None;
        }
        let previous = std::mem::replace(&mut self.current, std::ptr::null_mut());
        let result = do_async(|arg| unsafe {
            if previous.is_null() {
                spdk_bs_iter_first(self.store.ptr, Some(callback_with), arg);
            } else {
                spdk_bs_iter_next(self.store.ptr, previous, Some(callback_with), arg);
            }
        })
        .await;
        match result {
            Ok(ptr) => {
                self.current = ptr;
                let id = BlobId {
                    id: unsafe { spdk_blob_get_id(ptr) },
                };
                Some(self.store.open_blob(id).await)
            }
            Err(err) => {
                self.finished = true;
                if err.errcode() == -(nix::errno::Errno::ENOENT as i32) {
                    None
                } else {
                    Some(Err(err))
                }
            }
        }
    }
}

extern "C" fn close_on_drop(_: *mut c_void, errno: c_int) {
    if errno != 0 {
        error!("Failed to close blob: {}", errno);
    }
}

impl Drop for BlobIterator<'_> {
    fn drop(&mut self) {
        if !self.current.is_null() {
            unsafe { spdk_blob_close(self.current, Some(close_on_drop), std::ptr::null_mut()) };
        }
    }
}

impl Drop for Blob {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            unsafe { spdk_blob_close(self.ptr, Some(close_on_drop), std::ptr::null_mut()) };
        }
    }
}

impl Blob {
    pub fn set_xattr(&mut self, name: &str, value: &[u8]) -> Result<()> {
        let name = CString::new(name).map_err(|_| SpdkError::from(nix::errno::Errno::EINVAL))?;
        let length = value
            .len()
            .try_into()
            .map_err(|_| SpdkError::from(nix::errno::Errno::E2BIG))?;
        SpdkError::from_retval(unsafe {
            spdk_blob_set_xattr(self.ptr, name.as_ptr(), value.as_ptr().cast(), length)
        })
    }

    pub fn get_xattr_value<T>(&self, name: &str, f: impl FnOnce(&[u8]) -> T) -> Result<T> {
        let name = CString::new(name).map_err(|_| SpdkError::from(nix::errno::Errno::EINVAL))?;
        let mut value = std::ptr::null();
        let mut length = 0;
        SpdkError::from_retval(unsafe {
            spdk_blob_get_xattr_value(self.ptr, name.as_ptr(), &mut value, &mut length)
        })?;
        let value = if length == 0 {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(value.cast::<u8>(), length as usize) }
        };
        Ok(f(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offsets_are_converted_to_io_units() {
        assert_eq!(io_range(8192, 4096, 4096).unwrap(), (2, 1));
        assert_eq!(io_range(4096, 8192, 512).unwrap(), (8, 16));
    }

    #[test]
    fn invalid_io_ranges_are_rejected() {
        for (offset, length, unit) in [(1, 4096, 4096), (0, 1, 4096), (0, 0, 0), (u64::MAX, 1, 1)] {
            assert_eq!(
                io_range(offset, length, unit).unwrap_err().errcode(),
                -(nix::errno::Errno::EINVAL as i32)
            );
        }
    }
}
