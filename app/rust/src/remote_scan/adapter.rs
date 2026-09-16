use super::model::RemoteEntry;
use std::fmt;
#[flutter_rust_bridge::frb(ignore)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteScanError {
    NotFound,
    Forbidden,
    RateLimited { retry_after_ms: Option<u64> },
    TransientNetwork(String),
    MalformedResponse(String),
    RangeUnavailable,
    Cancelled,
    Unauthorized,
    Unsupported,
    Io(String),
    Provider(String),
}
impl fmt::Display for RemoteScanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for RemoteScanError {}
#[flutter_rust_bridge::frb(ignore)]
#[derive(Debug, Clone, Default)]
pub struct RemoteCapabilities {
    pub range_read: bool,
    pub pagination: bool,
}
#[flutter_rust_bridge::frb(ignore)]
pub trait RemoteProviderAdapter: Send + Sync {
    fn list(
        &self,
        path: &str,
        cursor: Option<&str>,
    ) -> Result<(Vec<RemoteEntry>, Option<String>), RemoteScanError>;
    fn read_range(&self, path: &str, offset: u64, length: u64) -> Result<Vec<u8>, RemoteScanError>;
    fn read_file_limited(&self, path: &str, max_bytes: u64) -> Result<Vec<u8>, RemoteScanError>;
    fn normalize_path(&self, path: &str) -> String;
    /// Register a logical-to-provider path mapping supplied by a caller that
    /// already fetched a directory listing. Providers with opaque ids (for
    /// example fid/pickcode based sources) override this so a seeded root
    /// listing can still be followed recursively without listing the root a
    /// second time.
    fn register_path(&self, _logical_path: &str, _provider_path: &str) {}
    fn capabilities(
        &self,
        path: &str,
        fingerprint: &str,
    ) -> Result<RemoteCapabilities, RemoteScanError>;
}
