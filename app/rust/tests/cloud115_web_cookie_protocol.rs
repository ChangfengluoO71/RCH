//! Model classification: LEGACY_WEB_PROTOCOL_MODEL.
//! Offline-only model for the historical clear-form 115 Cookie torrent flow;
//! it does not model the current p115client SSP request or claim wire compatibility.
//!
//! All HTTP targets are loopback mocks and all credentials are fixed test
//! strings. This test adapter is not linked into the application runtime.

#[path = "support/cloud115_web_cookie_protocol.rs"]
mod protocol;

use protocol::{
    fixture_torrent, MockReply, MockServer, ProtocolError, SubmitOutcome, WebCookieProtocol,
};

const TEST_COOKIE: &str = "UID=12345;SEID=mock-session";
const TEST_USER_ID: &str = "12345";
const TEST_MILLIS: u64 = 1_700_000_000_123;

#[test]
fn cookie_sign_upload_parse_and_bt_payload_are_modeled() {
    let torrent = fixture_torrent();
    let identity = protocol::torrent_identity(&torrent).unwrap();
    assert_ne!(identity.file_sha1, identity.info_hash);
    let server = MockServer::start(vec![
        MockReply::json(
            200,
            r#"{"state":true,"sign":"fixture-sign","time":"fixture-time"}"#,
        ),
        MockReply::json(
            200,
            r#"{"state":true,"data":{"file_id":"fixture-file-id"}}"#,
        ),
        MockReply::json(
            200,
            format!(
                r#"{{"state":true,"data":[{{"pickcode":"fixture-pickcode","sha":"{}"}}]}}"#,
                identity.file_sha1
            ),
        ),
        MockReply::json(
            200,
            format!(
                r#"{{"state":true,"info_hash":"{}","torrent_name":"sample","torrent_filelist_web":[{{"name":"001.jpg","size":4}},{{"name":"readme.nfo","size":2}}]}}"#,
                identity.info_hash
            ),
        ),
        MockReply::json(200, r#"{"state":true,"tasks":[]}"#),
        MockReply::json(200, r#"{"state":true}"#),
    ]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();

    let sign = adapter.fetch_sign_time().unwrap();
    let uploaded = adapter
        .upload_torrent(&torrent, "sample.torrent", "torrent-folder-id")
        .unwrap();
    let parsed = adapter
        .parse_torrent(&uploaded, TEST_USER_ID, &sign, &identity)
        .unwrap();
    assert_eq!(parsed.info_hash, identity.info_hash);
    let outcome = adapter
        .submit_once(&parsed, TEST_USER_ID, &sign, "sample", &[0])
        .unwrap();

    assert_eq!(outcome, SubmitOutcome::Accepted);
    let requests = server.finish();
    assert_eq!(requests.len(), 6);
    assert_eq!(requests[0].method, "GET");
    assert!(requests[1..].iter().all(|request| request.method == "POST"));
    assert!(requests.iter().all(|request| {
        request.header("cookie") == Some(TEST_COOKIE) && request.header("user-agent").is_some()
    }));
    assert!(requests[0].target.contains("ct=offline&ac=space"));
    assert!(requests[0].target.contains(&format!("_={TEST_MILLIS}")));
    assert!(requests[1]
        .body
        .windows(torrent.len())
        .any(|part| part == torrent.as_slice()));
    assert!(requests[1]
        .body
        .windows(b"torrent-folder-id".len())
        .any(|part| part == b"torrent-folder-id"));
    assert!(requests[2].body_text().contains("file_id=fixture-file-id"));
    assert!(requests[3]
        .body_text()
        .contains("pickcode=fixture-pickcode"));
    assert!(requests[3]
        .body_text()
        .contains(&format!("sha={}", identity.file_sha1.to_ascii_uppercase())));
    assert!(requests[3].body_text().contains("uid=12345"));
    assert!(requests[3].body_text().contains("sign=fixture-sign"));
    assert!(requests[3].body_text().contains("time=fixture-time"));
    assert!(requests[4].target.contains("ac=task_lists"));
    assert_eq!(requests[5].target, "/lixian/?ct=lixian&ac=add_task_bt");
    let task_form = requests[5].body_text();
    assert!(task_form.contains(&format!("info_hash={}", identity.info_hash)));
    assert!(task_form.contains("wanted=0"));
    assert!(task_form.contains("savepath=sample"));
    assert!(task_form.contains("uid=12345"));
    assert!(task_form.contains("sign=fixture-sign"));
    assert!(task_form.contains("time=fixture-time"));
    assert!(!task_form.contains("wp_path_id"));
}

#[test]
fn existing_infohash_skips_bt_write() {
    let info_hash = "existing-info-hash";
    let server = MockServer::start(vec![MockReply::json(
        200,
        r#"{"state":true,"tasks":[{"info_hash":"existing-info-hash"}]}"#,
    )]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();

    let outcome = adapter
        .submit_hash_once(
            info_hash,
            TEST_USER_ID,
            "fixture-sign",
            "fixture-time",
            "sample",
            &[0],
        )
        .unwrap();

    assert_eq!(outcome, SubmitOutcome::AlreadyPresent);
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].target.contains("ac=task_lists"));
}

#[test]
fn lost_bt_response_is_reconciled_before_any_second_post() {
    let server = MockServer::start(vec![
        MockReply::json(200, r#"{"state":true,"tasks":[]}"#),
        MockReply::close_without_response(),
        MockReply::json(
            200,
            r#"{"state":true,"tasks":[{"info_hash":"uncertain-hash"}]}"#,
        ),
    ]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();

    let first = adapter.submit_hash_once(
        "uncertain-hash",
        TEST_USER_ID,
        "fixture-sign",
        "fixture-time",
        "sample",
        &[0],
    );
    assert!(matches!(first, Err(ProtocolError::RemoteStateUnknown(_))));
    let recovered = adapter
        .recover_unknown(
            "uncertain-hash",
            TEST_USER_ID,
            "fixture-sign",
            "fixture-time",
        )
        .unwrap();

    assert_eq!(recovered, SubmitOutcome::AlreadyPresent);
    let requests = server.finish();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.contains("ac=add_task_bt"))
            .count(),
        1
    );
}

#[test]
fn upload_timeout_is_unknown_and_cannot_be_retried_without_reconciliation() {
    let torrent = fixture_torrent();
    let identity = protocol::torrent_identity(&torrent).unwrap();
    let server = MockServer::start(vec![MockReply::delay_without_response(
        std::time::Duration::from_millis(2300),
    )]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();

    let first = adapter.upload_torrent(&torrent, "sample.torrent", "folder-id");
    assert!(matches!(
        first,
        Err(ProtocolError::RemoteUploadStateUnknown(ref sha1)) if sha1 == &identity.file_sha1
    ));

    let retry = adapter.upload_torrent(&torrent, "sample.torrent", "folder-id");
    assert!(matches!(
        retry,
        Err(ProtocolError::RemoteUploadStateUnknown(ref sha1)) if sha1 == &identity.file_sha1
    ));
    let requests = server.finish();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].target, "/mock-upload");
}

#[test]
fn waf_405_stops_without_endpoint_fallback_or_retry() {
    let server = MockServer::start(vec![
        MockReply::json(200, r#"{"state":true,"tasks":[]}"#),
        MockReply::json(405, r#"{"state":false,"message":"blocked"}"#),
    ]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();

    let result = adapter.submit_hash_once(
        "waf-hash",
        TEST_USER_ID,
        "fixture-sign",
        "fixture-time",
        "sample",
        &[0],
    );

    assert!(matches!(result, Err(ProtocolError::WafBlocked)));
    let requests = server.finish();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.target.contains("ac=add_task_bt"))
            .count(),
        1
    );
}

#[test]
fn protocol_adapter_rejects_non_loopback_origins() {
    let result = WebCookieProtocol::new("http://example.com", TEST_COOKIE, TEST_MILLIS);
    assert!(result.is_err());
}

#[test]
fn wanted_indices_outside_parsed_file_list_are_rejected_before_network() {
    let server = MockServer::start(vec![]);
    let adapter = WebCookieProtocol::new(server.addr(), TEST_COOKIE, TEST_MILLIS).unwrap();
    let torrent = protocol::ParsedTorrent {
        info_hash: "fixture-info-hash".to_owned(),
        torrent_name: "sample".to_owned(),
        files: vec![
            protocol::TorrentFile {
                name: "001.jpg".to_owned(),
                size: 4,
            },
            protocol::TorrentFile {
                name: "readme.nfo".to_owned(),
                size: 2,
            },
        ],
    };

    let result = adapter.submit_once(
        &torrent,
        TEST_USER_ID,
        &protocol::SignTime {
            sign: "fixture-sign".to_owned(),
            time: "fixture-time".to_owned(),
        },
        "sample",
        &[2],
    );

    assert!(matches!(result, Err(ProtocolError::InvalidWantedIndex)));
    assert!(server.finish().is_empty());
}
