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
