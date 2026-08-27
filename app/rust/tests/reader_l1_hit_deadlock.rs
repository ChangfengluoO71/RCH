use anyhow::Result;
use rust_lib_app::document::{Document, DocumentMeta};
use rust_lib_app::reader::Reader;
use std::sync::{mpsc, Arc};
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

fn get_page_with_timeout(reader: &Arc<Reader>, index: u32) -> Vec<u8> {
    let (tx, rx) = mpsc::channel();
    let worker = Arc::clone(reader);
    std::thread::spawn(move || {
        let result = worker.get_page(index).map(|bytes| (*bytes).clone());
        let _ = tx.send(result);
    });

    rx.recv_timeout(Duration::from_millis(750))
        .expect("page read must not deadlock")
        .expect("page read should succeed")
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

    // The first page-1 read either waits for the prefetch or directly hits L1.
    // In either case it must finish, and after it finishes page 1 is definitely cached.
    assert_eq!(get_page_with_timeout(&reader, 1), vec![1]);

    // This second read is definitely an L1 hit. It must return instead of holding
    // the cache mutex while spawn_prefetch() tries to lock the same cache again.
    assert_eq!(get_page_with_timeout(&reader, 1), vec![1]);
}
