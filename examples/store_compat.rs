//! Exercise the data-node dependency API with an in-memory SPDK device.
use async_spdk::{blob::BlobStore, blob_bdev::BlobStoreBDev, env::DmaBuf, event, Result};
use futures::channel::oneshot;
use std::sync::{Arc, Mutex};

fn main() {
    let outcome = Arc::new(Mutex::new(None));
    let result = outcome.clone();
    let mask = std::env::var("SPDK_TEST_CORE_MASK").unwrap_or_else(|_| "[0]".into());
    event::AppOpts::new()
        .name("store_compat")
        .config_file("examples/store_compat.json")
        .reactor_mask(&mask)
        .no_pci(true)
        .no_huge(true)
        .memory_size_mb(512)
        .trace_entries(0)
        .app_start(async move {
            *result.lock().unwrap() = Some(check_store_api().await);
            event::app_stop();
        })
        .expect("SPDK application failed");
    outcome
        .lock()
        .unwrap()
        .take()
        .expect("Test did not run")
        .expect("Store API check failed");
    println!("Store API check passed");
}

async fn check_store_api() -> Result<()> {
    let core = async_spdk::env::get_current_core().unwrap();
    assert!(async_spdk::env::get_reactor_cores().contains(&core));
    assert!(BlobStoreBDev::list_all().contains(&"StoreTest".to_owned()));

    let (tx, rx) = oneshot::channel();
    std::thread::spawn(move || tx.send(42).unwrap());
    assert_eq!(
        event::spawn_local(async move { rx.await.unwrap() }).await,
        42
    );
    for reactor in async_spdk::env::get_reactor_cores() {
        let (tx, rx) = oneshot::channel();
        event::SpdkEvent::call_on(reactor, move || {
            event::spawn_local(async move {
                tx.send(async_spdk::env::get_current_core()).unwrap();
            })
            .detach();
        })?;
        assert_eq!(rx.await.unwrap(), Some(reactor));
    }

    let (tx, rx) = oneshot::channel();
    let sender = std::cell::RefCell::new(Some(tx));
    let poller = async_spdk::thread::SpdkPoller::new(move || {
        if let Some(tx) = sender.borrow_mut().take() {
            tx.send(()).unwrap();
            true
        } else {
            false
        }
    })?;
    rx.await.unwrap();
    drop(poller);

    let mut device = BlobStoreBDev::create("StoreTest")?;
    let mut store = BlobStore::init(&mut device).await?;
    let id = store.create_blob().await?;
    let mut blob = store.open_blob(id).await?;
    blob.resize(1).await?;
    blob.set_xattr("store_meta", b"volume-and-extent")?;
    blob.sync_metadata().await?;
    store.set_super(id).await?;
    assert_eq!(store.get_super().await?, id);

    let unit = store.io_unit_size() as usize;
    let channel = store.get_io_channel();
    let mut written = DmaBuf::alloc(unit, unit);
    for byte in written.as_mut() {
        byte.write(0x5a);
    }
    let written = unsafe { written.assume_init() };
    blob.write_zero(&channel, 0, 3 * unit as u64).await?;
    blob.write(&channel, 2 * unit as u64, &written).await?;
    let mut read = DmaBuf::alloc(unit, unit);
    blob.read(&channel, 2 * unit as u64, &mut read).await?;
    let read = unsafe { read.assume_init() };
    assert_eq!(written.as_ref(), read.as_ref());
    assert_eq!(
        blob.write(&channel, 1, &written)
            .await
            .unwrap_err()
            .errcode(),
        -22
    );
    drop(channel);
    blob.close().await?;
    store.unload().await?;

    let mut device = BlobStoreBDev::create("StoreTest")?;
    store = BlobStore::load(&mut device).await?;
    assert_eq!(store.get_super().await?, id);
    let mut iterator = store.list_blobs();
    let blob = iterator.next().await.unwrap()?;
    assert_eq!(blob.blob_id(), id);
    assert!(iterator.next().await.is_none());
    drop(iterator);
    assert_eq!(
        blob.get_xattr_value("store_meta", |v| v.to_vec())?,
        b"volume-and-extent"
    );
    let channel = store.get_io_channel();
    let mut read = DmaBuf::alloc(unit, unit);
    blob.read(&channel, 2 * unit as u64, &mut read).await?;
    let read = unsafe { read.assume_init() };
    assert_eq!(written.as_ref(), read.as_ref());
    drop(channel);
    blob.close().await?;
    store.delete_blob(id).await?;
    store.unload().await?;
    Ok(())
}
