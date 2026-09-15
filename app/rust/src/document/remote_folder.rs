use super::{Document, DocumentMeta};
use crate::remote_scan::adapter::{RemoteCapabilities, RemoteProviderAdapter, RemoteScanError};
use crate::remote_scan::model::{classify, is_ignored_name, normalize_path, RemoteAssetKind};
use anyhow::{anyhow, Result};
use std::fmt;
use std::sync::Arc;

pub const MAX_REMOTE_PAGE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteImageEntry {
    pub logical_path: String,
    pub name: String,
    pub size: Option<u64>,
    pub mtime: Option<i64>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteFolderError {
    EmptyManifest,
    PageOutOfRange { index: u32, page_count: u32 },
    PageTooLarge { size: u64, max: u64 },
    Cancelled,
}

impl fmt::Display for RemoteFolderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyManifest => write!(f, "remote image-folder manifest has no image pages"),
            Self::PageOutOfRange { index, page_count } => {
                write!(
                    f,
                    "remote folder page {index} is outside page count {page_count}"
                )
            }
            Self::PageTooLarge { size, max } => {
                write!(
                    f,
                    "remote folder page is too large ({size} bytes, maximum {max})"
                )
            }
            Self::Cancelled => write!(f, "remote folder page read was cancelled"),
        }
    }
}

impl std::error::Error for RemoteFolderError {}

pub trait RemoteFolderReader: Send + Sync {
    fn capabilities(
        &self,
        path: &str,
        fingerprint: &str,
    ) -> std::result::Result<RemoteCapabilities, RemoteScanError>;
    fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError>;
    fn read_file_limited(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError>;
    fn is_cancelled(&self) -> bool {
        false
    }
}

pub struct AdapterFolderReader {
    adapter: Arc<dyn RemoteProviderAdapter>,
}

impl AdapterFolderReader {
    pub fn new(adapter: Arc<dyn RemoteProviderAdapter>) -> Self {
        Self { adapter }
    }
}

impl RemoteFolderReader for AdapterFolderReader {
    fn capabilities(
        &self,
        path: &str,
        fingerprint: &str,
    ) -> std::result::Result<RemoteCapabilities, RemoteScanError> {
        self.adapter.capabilities(path, fingerprint)
    }

    fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError> {
        self.adapter.read_range(path, offset, length)
    }

    fn read_file_limited(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> std::result::Result<Vec<u8>, RemoteScanError> {
        self.adapter.read_file_limited(path, max_bytes)
    }
}

pub struct RemoteFolderBook {
    entries: Vec<RemoteImageEntry>,
    reader: Arc<dyn RemoteFolderReader>,
    meta: DocumentMeta,
}

impl RemoteFolderBook {
    pub fn open(
        entries: Vec<RemoteImageEntry>,
        reader: Arc<dyn RemoteFolderReader>,
        title: String,
    ) -> Result<Self> {
        let mut entries = entries
            .into_iter()
            .filter(|entry| {
                !is_ignored_name(&entry.name)
                    && classify(&entry.name, false) == RemoteAssetKind::ImageFile
            })
            .map(|mut entry| {
                entry.logical_path = normalize_path(&entry.logical_path);
                entry
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| crate::util::natural_cmp(&a.name, &b.name));
        if entries.is_empty() {
            return Err(RemoteFolderError::EmptyManifest.into());
        }
        Ok(Self {
            entries,
            reader,
            meta: DocumentMeta {
                title,
                ..DocumentMeta::default()
            },
        })
    }

    pub fn cover_entry(&self) -> Option<&RemoteImageEntry> {
        const COVER_NAMES: &[&str] = &["cover.jpg", "cover.png", "cover.webp", "cover.jpeg"];
        COVER_NAMES
            .iter()
            .find_map(|candidate| {
                self.entries
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(candidate))
            })
            .or_else(|| self.entries.first())
    }

    fn entry(&self, index: u32) -> Result<&RemoteImageEntry> {
        self.entries.get(index as usize).ok_or_else(|| {
            RemoteFolderError::PageOutOfRange {
                index,
                page_count: self.entries.len() as u32,
            }
            .into()
        })
    }
}

impl Document for RemoteFolderBook {
    fn page_count(&self) -> u32 {
        self.entries.len() as u32
    }

    fn metadata(&self) -> DocumentMeta {
        self.meta.clone()
    }

    fn page_bytes(&self, index: u32) -> Result<Vec<u8>> {
        let entry = self.entry(index)?;
        if self.reader.is_cancelled() {
            return Err(RemoteFolderError::Cancelled.into());
        }
        if let Some(size) = entry.size {
            if size > MAX_REMOTE_PAGE_BYTES {
                return Err(RemoteFolderError::PageTooLarge {
                    size,
                    max: MAX_REMOTE_PAGE_BYTES,
                }
                .into());
            }
        }

        let capabilities = self
            .reader
            .capabilities(&entry.logical_path, &entry.fingerprint)
            .map_err(|error| anyhow!(error))?;
        if self.reader.is_cancelled() {
            return Err(RemoteFolderError::Cancelled.into());
        }
        let bytes = if capabilities.range_read {
            let length = entry.size.unwrap_or(MAX_REMOTE_PAGE_BYTES + 1);
            self.reader
                .read_range(&entry.logical_path, 0, length)
                .map_err(|error| anyhow!(error))?
        } else {
            self.reader
                .read_file_limited(&entry.logical_path, MAX_REMOTE_PAGE_BYTES)
                .map_err(|error| anyhow!(error))?
        };
        if bytes.len() as u64 > MAX_REMOTE_PAGE_BYTES {
            return Err(RemoteFolderError::PageTooLarge {
                size: bytes.len() as u64,
                max: MAX_REMOTE_PAGE_BYTES,
            }
            .into());
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RemoteFolderBook, RemoteFolderError, RemoteFolderReader, RemoteImageEntry,
        MAX_REMOTE_PAGE_BYTES,
    };
    use crate::document::Document;
    use crate::remote_scan::adapter::{RemoteCapabilities, RemoteScanError};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeReader {
        pages: HashMap<String, Vec<u8>>,
        range: bool,
        cancelled: bool,
        reads: AtomicUsize,
        paths: Mutex<Vec<String>>,
    }

    impl FakeReader {
        fn new(pages: impl IntoIterator<Item = (&'static str, &'static [u8])>) -> Self {
            Self {
                pages: pages
                    .into_iter()
                    .map(|(path, bytes)| (path.to_string(), bytes.to_vec()))
                    .collect(),
                range: true,
                cancelled: false,
                reads: AtomicUsize::new(0),
                paths: Mutex::new(Vec::new()),
            }
        }
    }

    impl RemoteFolderReader for FakeReader {
        fn capabilities(
            &self,
            _path: &str,
            _fingerprint: &str,
        ) -> Result<RemoteCapabilities, RemoteScanError> {
            Ok(RemoteCapabilities {
                range_read: self.range,
                pagination: false,
            })
        }

        fn read_range(
            &self,
            path: &str,
            offset: u64,
            length: u64,
        ) -> Result<Vec<u8>, RemoteScanError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.paths.lock().unwrap().push(path.to_string());
            let bytes = self.pages.get(path).ok_or(RemoteScanError::NotFound)?;
            let start = usize::try_from(offset).unwrap();
            let end = start.saturating_add(length as usize).min(bytes.len());
            Ok(bytes[start..end].to_vec())
        }

        fn read_file_limited(
            &self,
            path: &str,
            max_bytes: u64,
        ) -> Result<Vec<u8>, RemoteScanError> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.paths.lock().unwrap().push(path.to_string());
            let bytes = self.pages.get(path).ok_or(RemoteScanError::NotFound)?;
            Ok(bytes[..bytes.len().min(max_bytes as usize + 1)].to_vec())
        }

        fn is_cancelled(&self) -> bool {
            self.cancelled
        }
    }

    fn entry(path: &str, name: &str, size: Option<u64>) -> RemoteImageEntry {
        RemoteImageEntry {
            logical_path: path.to_string(),
            name: name.to_string(),
            size,
            mtime: Some(7),
            fingerprint: format!("fp:{name}"),
        }
    }

    #[test]
    fn remote_folder_has_stable_natural_page_order_and_cover() {
        let reader = Arc::new(FakeReader::new([
            ("/book/page10.jpg", b"ten" as &[u8]),
            ("/book/cover.jpg", b"cover"),
            ("/book/page2.jpg", b"two"),
            ("/book/.hidden.jpg", b"hidden"),
        ]));
        let book = RemoteFolderBook::open(
            vec![
                entry("/book/page10.jpg", "page10.jpg", Some(3)),
                entry("/book/.hidden.jpg", ".hidden.jpg", Some(6)),
                entry("/book/page2.jpg", "page2.jpg", Some(3)),
                entry("/book/cover.jpg", "cover.jpg", Some(5)),
            ],
            reader.clone(),
            "Book".into(),
        )
        .unwrap();

        assert_eq!(book.page_count(), 3);
        assert_eq!(book.cover_entry().unwrap().name, "cover.jpg");
        assert_eq!(book.page_bytes(0).unwrap(), b"cover");
        assert_eq!(book.page_bytes(1).unwrap(), b"two");
        assert_eq!(book.page_bytes(2).unwrap(), b"ten");
        assert_eq!(
            reader.paths.lock().unwrap().as_slice(),
            ["/book/cover.jpg", "/book/page2.jpg", "/book/page10.jpg"]
        );
    }

    #[test]
    fn remote_folder_rejects_out_of_range_without_remote_read() {
        let reader = Arc::new(FakeReader::new([("/book/1.jpg", b"one" as &[u8])]));
        let book = RemoteFolderBook::open(
            vec![entry("/book/1.jpg", "1.jpg", Some(3))],
            reader.clone(),
            "Book".into(),
        )
        .unwrap();

        assert!(matches!(
            book.page_bytes(1)
                .unwrap_err()
                .downcast_ref::<RemoteFolderError>(),
            Some(RemoteFolderError::PageOutOfRange {
                index: 1,
                page_count: 1
            })
        ));
        assert_eq!(reader.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn remote_folder_checks_cancellation_before_remote_read() {
        let mut fake = FakeReader::new([("/book/1.jpg", b"one" as &[u8])]);
        fake.cancelled = true;
        let reader = Arc::new(fake);
        let book = RemoteFolderBook::open(
            vec![entry("/book/1.jpg", "1.jpg", Some(3))],
            reader.clone(),
            "Book".into(),
        )
        .unwrap();

        assert!(matches!(
            book.page_bytes(0)
                .unwrap_err()
                .downcast_ref::<RemoteFolderError>(),
            Some(RemoteFolderError::Cancelled)
        ));
        assert_eq!(reader.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn remote_folder_rejects_known_oversized_page_before_remote_read() {
        let reader = Arc::new(FakeReader::new(std::iter::empty()));
        let book = RemoteFolderBook::open(
            vec![entry(
                "/book/huge.jpg",
                "huge.jpg",
                Some(MAX_REMOTE_PAGE_BYTES + 1),
            )],
            reader.clone(),
            "Book".into(),
        )
        .unwrap();

        assert!(matches!(
            book.page_bytes(0)
                .unwrap_err()
                .downcast_ref::<RemoteFolderError>(),
            Some(RemoteFolderError::PageTooLarge { .. })
        ));
        assert_eq!(reader.reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn remote_folder_non_range_path_is_bounded_to_one_image_response() {
        let mut fake = FakeReader::new([("/book/1.jpg", b"one" as &[u8])]);
        fake.range = false;
        let reader = Arc::new(fake);
        let book = RemoteFolderBook::open(
            vec![entry("/book/1.jpg", "1.jpg", None)],
            reader.clone(),
            "Book".into(),
        )
        .unwrap();

        assert_eq!(book.page_bytes(0).unwrap(), b"one");
        assert_eq!(reader.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn remote_folder_types_unknown_oversized_single_image_response() {
        let bytes = vec![7; MAX_REMOTE_PAGE_BYTES as usize + 1];
        let reader = Arc::new(FakeReader {
            pages: HashMap::from([("/book/huge.jpg".into(), bytes)]),
            range: false,
            cancelled: false,
            reads: AtomicUsize::new(0),
            paths: Mutex::new(Vec::new()),
        });
        let book = RemoteFolderBook::open(
            vec![entry("/book/huge.jpg", "huge.jpg", None)],
            reader,
            "Book".into(),
        )
        .unwrap();

        assert!(matches!(
            book.page_bytes(0)
                .unwrap_err()
                .downcast_ref::<RemoteFolderError>(),
            Some(RemoteFolderError::PageTooLarge { .. })
        ));
    }
}
