use super::model::RemoteEntry;
use std::fmt;
#[flutter_rust_bridge::frb(ignore)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteScanError {
    NotFound,
    Forbidden,
    RateLimited {
        retry_after_ms: Option<u64>,
    },
    TransientNetwork(String),
    MalformedResponse(String),
    RangeUnavailable,
    Cancelled,
    Unauthorized,
    Unsupported,
    Io(String),
    Provider(String),
    /// An HTTP response that carries useful protocol information but does not
    /// map to one of the provider-independent categories above.  Keeping the
    /// stage and status lets the UI distinguish WAF/endpoint failures from a
    /// genuine lack of Range support without persisting response bodies.
    HttpStatus {
        stage: String,
        status: u16,
    },
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

/// Result of a bounded `bytes=0-0` probe.  A successful probe is only one
/// that returned `206 Partial Content` with a valid `Content-Range` header.
/// Providers that answer with a full `200` response are reported as
/// unsupported, not as a transport error.
#[flutter_rust_bridge::frb(ignore)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeProbe {
    pub supported: bool,
    pub total_size: Option<u64>,
}

/// Classify a provider's Range probe response without exposing body contents.
/// This pure boundary is shared by provider adapters and deliberately keeps
/// HTTP status information for authentication/WAF/transient failures.
pub fn classify_range_probe_response(
    status: u16,
    content_range: Option<&str>,
    content_length: Option<u64>,
) -> Result<RangeProbe, RemoteScanError> {
    match status {
        401 => return Err(RemoteScanError::Unauthorized),
        403 => return Err(RemoteScanError::Forbidden),
        404 => return Err(RemoteScanError::NotFound),
        405 => {
            return Err(RemoteScanError::HttpStatus {
                stage: "range_probe".into(),
                status,
            })
        }
        408 | 425 | 429 | 500..=599 => {
            return Err(if status == 429 {
                RemoteScanError::RateLimited {
                    retry_after_ms: None,
                }
            } else {
                RemoteScanError::TransientNetwork(format!("range_probe_http_{status}"))
            })
        }
        _ => {}
    }

    if status == 206 {
        let Some(header) = content_range else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        let Some((unit, value)) = header.split_once(' ') else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        if !unit.eq_ignore_ascii_case("bytes") {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        }
        let Some((range, total)) = value.split_once('/') else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        let Some((start, end)) = range.split_once('-') else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        let (start, end) = (start.trim().parse::<u64>(), end.trim().parse::<u64>());
        let (Ok(start), Ok(end)) = (start, end) else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        let Ok(total) = total.trim().parse::<u64>() else {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        };
        if start != 0 || end != 0 || total == 0 {
            return Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into(),
            ));
        }
        return Ok(RangeProbe {
            supported: true,
            total_size: Some(total),
        });
    }

    // A server may ignore Range and return the complete representation.  It
    // is a valid response, but it cannot be used for bounded reads.
    Ok(RangeProbe {
        supported: false,
        total_size: content_length.filter(|size| *size > 0),
    })
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
    /// Return a provider-facing cache alias for a logical path when the
    /// provider uses opaque identifiers.  The scanner always writes the
    /// canonical logical key as well; this optional alias lets existing UI
    /// calls (which still pass the provider id) hit the scanner-produced
    /// cover without triggering another network request.
    fn cache_path(&self, logical_path: &str) -> Option<String> {
        Some(self.normalize_path(logical_path))
    }
    fn capabilities(
        &self,
        path: &str,
        fingerprint: &str,
    ) -> Result<RemoteCapabilities, RemoteScanError>;
}
