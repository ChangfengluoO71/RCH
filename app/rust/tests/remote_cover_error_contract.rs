use rust_lib_app::remote_scan::adapter::{
    classify_range_probe_response, RangeProbe, RemoteScanError,
};

#[test]
fn range_probe_requires_valid_partial_content() {
    assert_eq!(
        classify_range_probe_response(206, Some("bytes 0-0/1024"), Some(1)).unwrap(),
        RangeProbe {
            supported: true,
            total_size: Some(1024),
        }
    );

    assert_eq!(
        classify_range_probe_response(200, None, Some(1024)).unwrap(),
        RangeProbe {
            supported: false,
            total_size: Some(1024),
        }
    );

    assert!(matches!(
        classify_range_probe_response(206, Some("bytes 1-1/1024"), Some(1)),
        Err(RemoteScanError::MalformedResponse(code)) if code == "range_probe_content_range"
    ));
    assert!(matches!(
        classify_range_probe_response(206, None, Some(1)),
        Err(RemoteScanError::MalformedResponse(code)) if code == "range_probe_content_range"
    ));
}

#[test]
fn provider_http_failures_keep_stage_and_status() {
    assert_eq!(
        classify_range_probe_response(401, None, None),
        Err(RemoteScanError::Unauthorized)
    );
    assert_eq!(
        classify_range_probe_response(403, None, None),
        Err(RemoteScanError::Forbidden)
    );
    assert_eq!(
        classify_range_probe_response(405, None, None),
        Err(RemoteScanError::HttpStatus {
            stage: "range_probe".into(),
            status: 405,
        })
    );
    assert!(matches!(
        classify_range_probe_response(503, None, None),
        Err(RemoteScanError::TransientNetwork(code)) if code == "range_probe_http_503"
    ));
}

// ---------------------------------------------------------------------------
// RG-A（Release Gate A 类：可自动化/可重复验证）
// ---------------------------------------------------------------------------

/// RG-A A-3：**403 / 405 / 429 是三种不同语义**，不得互相坍缩。
///
/// * 403 ⇒ `Forbidden`（凭证/权限被拒；UI = 暂不可用）
/// * 405 ⇒ `HttpStatus{stage:"range_probe", status:405}`
///   （保留协议信息，以便区分 WAF / 端点问题 与"真的不支持 Range"）
/// * 429 ⇒ `RateLimited{retry_after_ms}`（**唯一**进入限流退避契约的分支）
///
/// 本用例的存在意义：把"**处理语义正确**（可自动化，本用例）"与
/// "**真实 115/Quark 未出现新的 403/405/429**（只能 RG-B 证明）"分开 ——
/// 两条证据不可合并。
#[test]
fn rg_a_a3_forbidden_method_not_allowed_and_rate_limited_stay_distinct() {
    assert_eq!(
        classify_range_probe_response(403, None, None),
        Err(RemoteScanError::Forbidden)
    );
    assert_eq!(
        classify_range_probe_response(405, None, None),
        Err(RemoteScanError::HttpStatus {
            stage: "range_probe".into(),
            status: 405,
        })
    );
    assert_eq!(
        classify_range_probe_response(429, None, None),
        Err(RemoteScanError::RateLimited {
            retry_after_ms: None,
        })
    );

    // 互不相等：防止未来把 405 并进 Forbidden，或把 403 当成可重试。
    assert_ne!(
        classify_range_probe_response(403, None, None),
        classify_range_probe_response(405, None, None)
    );
    assert_ne!(
        classify_range_probe_response(405, None, None),
        classify_range_probe_response(429, None, None)
    );
    assert_ne!(
        classify_range_probe_response(403, None, None),
        classify_range_probe_response(429, None, None)
    );
}

/// RG-A A-3：其它瞬时状态码归入有界的 `TransientNetwork` 桶，并保留精确错误码
/// （`range_probe_http_<status>`），使退避路径可诊断。
#[test]
fn rg_a_a3_transient_status_codes_map_to_bounded_backoff_bucket() {
    for status in [408_u16, 425, 500, 502, 503, 599] {
        assert_eq!(
            classify_range_probe_response(status, None, None),
            Err(RemoteScanError::TransientNetwork(format!(
                "range_probe_http_{status}"
            ))),
            "status {status} must be classified as transient"
        );
    }
    // 429 不得落进 transient 桶（它必须可携带 Retry-After 语义）。
    assert!(matches!(
        classify_range_probe_response(429, None, None),
        Err(RemoteScanError::RateLimited { .. })
    ));
}

/// RG-A A-2：206 但 `Content-Range` 不可信 ⇒ `MalformedResponse`
/// （**不是**静默降级为"不支持 Range"）。
#[test]
fn rg_a_a2_untrustworthy_partial_content_is_malformed() {
    for header in [
        None,
        Some("bytes 0-0"),
        Some("items 0-0/1024"),
        Some("bytes x-y/1024"),
    ] {
        assert_eq!(
            classify_range_probe_response(206, header, Some(1024)),
            Err(RemoteScanError::MalformedResponse(
                "range_probe_content_range".into()
            )),
            "header {header:?} must be rejected as malformed"
        );
    }
}

/// RG-A A-2：纯 200（无 206 语义）⇒ `supported = false` ——
/// 这是 ADR-005「不支持 Range 时整包下载到本地缓存」的回退触发条件。
#[test]
fn rg_a_a2_plain_200_marks_range_unsupported_for_adr005_fallback() {
    assert_eq!(
        classify_range_probe_response(200, None, Some(4096)).unwrap(),
        RangeProbe {
            supported: false,
            total_size: Some(4096),
        }
    );
    assert_eq!(
        classify_range_probe_response(200, None, None).unwrap(),
        RangeProbe {
            supported: false,
            total_size: None,
        }
    );
}
