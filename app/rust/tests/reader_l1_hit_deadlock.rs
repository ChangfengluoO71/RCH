use rust_lib_app::document::{Document, DocumentMeta};
use rust_lib_app::reader::Reader;
use anyhow::Result;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct CachedDoc;

impl Document for CachedDoc {
    fn page_count(&self) -> u32 {
        2
    }

    fn metadata(&self) -> DocumentMeta {
        DocumentMeta {
            title: "cached-test".to_string(),
            ..Default::default()
        }
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        Ok(vec![index as u8])
    }
}

#[test]
fn l1_hit_does_not_deadlock_while_starting_prefetch() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let reader = Arc::new(Reader::new(
        Box::new(CachedDoc),
        &format!("reader-l1-hit-deadlock-{nonce}"),
    ));

    // First read fills page 0 and starts nearby prefetch.
    assert_eq!(&*reader.get_page(0).unwrap(), &[0]);
    // This read waits for or completes page 1 and guarantees it is now in L1.
    assert_eq!(&*reader.get_page(1).unwrap(), &[1]);

    // A second read of page 1 must be a direct L1 hit and must not self-deadlock
    // while get_page() starts prefetch around the cached page.
    let (tx, rx) = mpsc::channel();
    let worker = Arc::clone(&reader);
    std::thread::spawn(move || {
        let result = worker.get_page(1).map(|bytes| (*bytes).clone());
        let _ = tx.send(result);
    });

    let result = rx
        .recv_timeout(Duration::from_millis(250))
        .expect("L1 cache hit must not deadlock while starting prefetch")
        .expect("cached page should load");
    assert_eq!(result, vec![1]);
}
