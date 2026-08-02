//! 本地文件系统书源。

use super::{ByteSource, Entry};
use std::fs::{self, File};
use std::io;
use std::path::Path;

/// 本地文件作为 [`ByteSource`]。
pub struct LocalFile {
    file: File,
    len: u64,
}

impl LocalFile {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let file = File::open(path)?;
        let len = file.metadata()?.len();
        Ok(LocalFile { file, len })
    }
}

impl ByteSource for LocalFile {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        read_at(&self.file, offset, buf)
    }
}

#[cfg(windows)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::windows::fs::FileExt;
    file.seek_read(buf, offset)
}

#[cfg(unix)]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::os::unix::fs::FileExt;
    file.read_at(buf, offset)
}

/// 兜底(其他平台):克隆句柄 + seek + read。
#[cfg(not(any(windows, unix)))]
fn read_at(file: &File, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = file.try_clone()?;
    f.seek(SeekFrom::Start(offset))?;
    f.read(buf)
}

/// 列出目录内容(书架浏览):目录在前,按自然排序。
pub fn list_dir(path: &str) -> io::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in fs::read_dir(path)? {
        let e = e?;
        let md = e.metadata()?;
        out.push(Entry {
            name: e.file_name().to_string_lossy().into_owned(),
            path: e.path().to_string_lossy().into_owned(),
            is_dir: md.is_dir(),
            size: md.len(),
            mtime: md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| crate::util::natural_cmp(&a.name, &b.name))
    });
    Ok(out)
}
