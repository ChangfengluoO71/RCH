//! Non-blocking provider/account budget for background cover work.
//!
//! Provider clients already have their protocol-specific rate gates.  This
//! small layer coordinates the queue before it takes a governor permit so a
//! worker does not occupy a reader slot while waiting for an account cooldown.
//! The key is deliberately supplied by the caller and must be a non-secret
//! account identity (credential reference, app id, or a conservative source
//! id fallback), never a Cookie, token, or direct URL.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetDecision {
    Granted,
    WaitUntil(i64),
}

#[derive(Debug)]
pub struct ProviderBudget {
    min_interval_ms: i64,
    next_by_account: Mutex<HashMap<String, i64>>,
}

impl ProviderBudget {
    pub fn new(min_interval_ms: i64) -> Self {
        Self {
            min_interval_ms: min_interval_ms.max(0),
            next_by_account: Mutex::new(HashMap::new()),
        }
    }

    /// Reserve the next request opportunity without sleeping while holding a
    /// queue or reader permit.  Callers can release their durable lease and
    /// sleep until the returned deadline before trying again.
    pub fn try_reserve(&self, account_key: &str, now_ms: i64) -> BudgetDecision {
        let key = account_key.trim();
        if key.is_empty() || self.min_interval_ms == 0 {
            return BudgetDecision::Granted;
        }
        let mut next = self.next_by_account.lock().unwrap();
        let allowed_at = next.get(key).copied().unwrap_or(now_ms);
        if allowed_at > now_ms {
            return BudgetDecision::WaitUntil(allowed_at);
        }
        next.insert(key.to_string(), now_ms.saturating_add(self.min_interval_ms));
        BudgetDecision::Granted
    }

    pub fn forget(&self, account_key: &str) {
        self.next_by_account.lock().unwrap().remove(account_key);
    }
}

/// Build a stable, non-secret budget key.  `account_identity` must already be
/// sanitized by the caller; the fallback is intentionally source-scoped when
/// a provider cannot expose an account id.
pub fn account_key(provider: &str, account_identity: &str, credential_epoch: &str) -> String {
    let identity = if account_identity.trim().is_empty() {
        "source"
    } else {
        account_identity.trim()
    };
    let epoch = if credential_epoch.trim().is_empty() {
        "current"
    } else {
        credential_epoch.trim()
    };
    format!(
        "{}|{}|{}",
        provider.trim().to_ascii_lowercase(),
        identity,
        epoch
    )
}

pub fn global() -> &'static ProviderBudget {
    static GLOBAL: OnceLock<ProviderBudget> = OnceLock::new();
    // 100ms is only the cross-task fairness floor. Provider clients retain
    // their stricter API/WAF gates, so this does not weaken existing limits.
    GLOBAL.get_or_init(|| ProviderBudget::new(100))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_key_never_uses_empty_or_case_sensitive_provider() {
        assert_eq!(account_key("115", "account", "epoch"), "115|account|epoch");
        assert_eq!(account_key("115", "", ""), "115|source|current");
    }

    #[test]
    fn reservation_is_non_blocking_and_has_a_durable_deadline() {
        let budget = ProviderBudget::new(100);
        assert_eq!(budget.try_reserve("115|a", 10), BudgetDecision::Granted);
        assert_eq!(
            budget.try_reserve("115|a", 50),
            BudgetDecision::WaitUntil(110)
        );
        assert_eq!(budget.try_reserve("115|b", 50), BudgetDecision::Granted);
        assert_eq!(budget.try_reserve("115|a", 110), BudgetDecision::Granted);
        budget.forget("115|a");
        assert_eq!(budget.try_reserve("115|a", 110), BudgetDecision::Granted);
    }
}
