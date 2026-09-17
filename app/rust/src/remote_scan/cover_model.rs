//! Shared value types for the remote catalog/cover pipeline.
//!
//! The database stores the string representation so older builds can still
//! inspect the tables.  Keeping the state conversion here prevents the UI and
//! workers from inventing subtly different spellings.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverJobState {
    Pending,
    Running,
    Ready,
    RetryWait,
    Blocked,
    Unsupported,
    Failed,
    Cancelled,
}

impl CoverJobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::RetryWait => "retry_wait",
            Self::Blocked => "blocked",
            Self::Unsupported => "unsupported",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "pending" => Self::Pending,
            "running" => Self::Running,
            "ready" => Self::Ready,
            "retry_wait" => Self::RetryWait,
            "blocked" => Self::Blocked,
            "unsupported" => Self::Unsupported,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoverJobKey {
    pub source_id: String,
    pub asset_id: String,
    pub content_revision: String,
    pub selection_revision: String,
    pub profile: String,
}

impl CoverJobKey {
    pub fn encode(&self) -> String {
        // Length-prefixing avoids collisions when a path contains the
        // separator.  The resulting value is an opaque database key.
        [
            self.source_id.as_str(),
            self.asset_id.as_str(),
            self.content_revision.as_str(),
            self.selection_revision.as_str(),
            self.profile.as_str(),
        ]
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join("|")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCoverJob {
    pub key: CoverJobKey,
    pub state: CoverJobState,
    pub demand_kind: String,
    pub priority: i64,
    pub attempt: i64,
    pub next_attempt_at: Option<i64>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<i64>,
    pub generation: i64,
    pub session_epoch: String,
    pub error_code: Option<String>,
    pub updated_at: i64,
}
