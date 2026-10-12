//! Retired 115 transfer probe.
//!
//! The previous version guessed several write endpoints and issued requests
//! that could create or clear real cloud tasks. It also used an unrelated
//! credential supplied through environment variables. That behavior is not a
//! safe way to verify an official API contract or reuse RCH's existing login
//! session.
//!
//! This example intentionally performs no network requests. It cannot borrow an
//! in-memory session from a running RCH process. A real transfer requires an
//! official 115 contract and one authorized test Torrent. Never probe candidate
//! write routes.

fn main() {
    eprintln!("cloud115_offline_probe is disabled: it cannot access RCH's in-memory 115 session.");
    eprintln!("It accepts no credentials and performs no HTTP requests or cloud writes.");
}

// In-memory contract flow for tests only; this is not an HTTP adapter.
#[cfg(test)]
mod contract {
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct TorrentFile {
        pub path: String,
        pub size: u64,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct ParsedTorrent {
        pub info_hash: String,
        pub files: Vec<TorrentFile>,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct AddTaskBtRequest {
        pub info_hash: String,
        pub wanted: String,
        pub save_path: String,
        pub torrent_sha1: String,
        pub pick_code: String,
        pub wp_path_id: String,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) struct RemoteTask {
        pub info_hash: String,
        pub wp_path_id: String,
        pub status: i32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum UploadReply {
        Uploaded,
        Unknown,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) enum MockReply {
        Accepted,
        Unknown,
        Rejected,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum MockCall {
        Upload {
            bytes: Vec<u8>,
            torrent_sha1: String,
            target_folder_id: String,
            endpoints: Vec<&'static str>,
        },
        Parse {
            torrent_sha1: String,
            pick_code: String,
            endpoint: &'static str,
        },
        Submit {
            request: AddTaskBtRequest,
            endpoint: &'static str,
        },
        ListTasks {
            page: u32,
            endpoint: &'static str,
        },
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(super) enum ContractOutcome {
        RemoteTaskFound { status: i32 },
        RemoteStateUnknown,
        ExplicitlyRejected,
    }

    pub(super) trait Mock115Api {
        fn upload_torrent(
            &mut self,
            torrent: &[u8],
            torrent_sha1: &str,
            target_folder_id: &str,
        ) -> Result<(UploadReply, String), String>;

        fn parse_torrent(
            &mut self,
            torrent_sha1: &str,
            pick_code: &str,
        ) -> Result<ParsedTorrent, String>;

        fn add_task_bt(&mut self, request: AddTaskBtRequest) -> MockReply;

        fn get_task_list(&mut self, page: u32) -> Result<Vec<RemoteTask>, String>;
    }

    pub(super) fn run_single_torrent(
        api: &mut impl Mock115Api,
        torrent: &[u8],
        torrent_sha1: &str,
        expected_info_hash: &str,
        target_folder_id: &str,
        wanted_indexes: &[usize],
    ) -> Result<ContractOutcome, String> {
        if torrent.is_empty() {
            return Err("torrent file is empty".to_string());
        }
        if !is_sha1_hex(torrent_sha1) || !is_sha1_hex(expected_info_hash) {
            return Err("torrent SHA-1 or info hash is invalid".to_string());
        }
        if target_folder_id.is_empty() {
            return Err("target folder ID is empty".to_string());
        }
        if wanted_indexes.is_empty() {
            return Err("no Torrent files were explicitly selected".to_string());
        }

        let (upload_reply, pick_code) =
            api.upload_torrent(torrent, torrent_sha1, target_folder_id)?;
        if upload_reply == UploadReply::Unknown {
            // The upload result is unknown. This flow has no safe upload lookup
            // identifier, so it stops without parsing, submitting, or retrying.
            return Ok(ContractOutcome::RemoteStateUnknown);
        }
        if pick_code.is_empty() {
            return Err("uploaded Torrent has no pick_code".to_string());
        }

        let parsed = api.parse_torrent(torrent_sha1, &pick_code)?;
        if !is_sha1_hex(&parsed.info_hash)
            || !parsed.info_hash.eq_ignore_ascii_case(expected_info_hash)
        {
            return Err("115 parsed info_hash does not match the local Torrent".to_string());
        }

        let mut indexes = wanted_indexes.to_vec();
        indexes.sort_unstable();
        indexes.dedup();
        if indexes.len() != wanted_indexes.len() || indexes.iter().any(|i| *i >= parsed.files.len())
        {
            return Err(
                "wanted indexes must be unique and present in the parsed file list".to_string(),
            );
        }
        if indexes
            .iter()
            .any(|index| parsed.files[*index].path.trim().is_empty())
        {
            return Err("selected Torrent entry has an empty path".to_string());
        }
        let wanted = indexes
            .iter()
            .map(usize::to_string)
            .collect::<Vec<_>>()
            .join(",");

        let request = AddTaskBtRequest {
            info_hash: parsed.info_hash,
            wanted,
            save_path: String::new(),
            torrent_sha1: torrent_sha1.to_string(),
            pick_code,
            wp_path_id: target_folder_id.to_string(),
        };
        let submission = api.add_task_bt(request);
        if submission == MockReply::Rejected {
            return Ok(ContractOutcome::ExplicitlyRejected);
        }

        // Even after an accepted response, confirm by info_hash and destination.
        // An unknown response follows the same read-only reconciliation path and
        // is never submitted again by this probe flow.
        let tasks = api.get_task_list(1)?;
        match tasks.into_iter().find(|task| {
            task.info_hash.eq_ignore_ascii_case(expected_info_hash)
                && task.wp_path_id == target_folder_id
        }) {
            Some(task) => Ok(ContractOutcome::RemoteTaskFound {
                status: task.status,
            }),
            None => Ok(ContractOutcome::RemoteStateUnknown),
        }
    }

    fn is_sha1_hex(value: &str) -> bool {
        value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }
}

#[cfg(test)]
mod contract_tests {
    use super::contract::{
        run_single_torrent, AddTaskBtRequest, ContractOutcome, Mock115Api, MockCall, MockReply,
        ParsedTorrent, RemoteTask, TorrentFile, UploadReply,
    };

    struct Mock115 {
        calls: Vec<MockCall>,
        upload_reply: UploadReply,
        reply: MockReply,
    }

    impl Mock115Api for Mock115 {
        fn upload_torrent(
            &mut self,
            torrent: &[u8],
            torrent_sha1: &str,
            target_folder_id: &str,
        ) -> Result<(UploadReply, String), String> {
            self.calls.push(MockCall::Upload {
                bytes: torrent.to_vec(),
                torrent_sha1: torrent_sha1.to_string(),
                target_folder_id: target_folder_id.to_string(),
                endpoints: vec![
                    "GET /open/upload/get_token",
                    "POST /open/upload/init",
                    "OSS object upload with callback",
                ],
            });
            Ok((self.upload_reply, "mock-pick-code".to_string()))
        }

        fn parse_torrent(
            &mut self,
            torrent_sha1: &str,
            pick_code: &str,
        ) -> Result<ParsedTorrent, String> {
            self.calls.push(MockCall::Parse {
                torrent_sha1: torrent_sha1.to_string(),
                pick_code: pick_code.to_string(),
                endpoint: "POST /open/offline/torrent",
            });
            Ok(ParsedTorrent {
                info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
                files: vec![TorrentFile {
                    path: "authorized-test.cbz".to_string(),
                    size: 42,
                }],
            })
        }

        fn add_task_bt(&mut self, request: AddTaskBtRequest) -> MockReply {
            self.calls.push(MockCall::Submit {
                request,
                endpoint: "POST /open/offline/add_task_bt",
            });
            self.reply
        }

        fn get_task_list(&mut self, page: u32) -> Result<Vec<RemoteTask>, String> {
            self.calls.push(MockCall::ListTasks {
                page,
                endpoint: "GET /open/offline/get_task_list",
            });
            Ok(match self.reply {
                MockReply::Accepted => vec![RemoteTask {
                    info_hash: "0123456789abcdef0123456789abcdef01234567".to_string(),
                    wp_path_id: "42".to_string(),
                    status: 1,
                }],
                MockReply::Unknown | MockReply::Rejected => Vec::new(),
            })
        }
    }

    const TORRENT_BYTES: &[u8] = b"mock authorized torrent bytes";
    const TORRENT_SHA1: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const INFO_HASH: &str = "0123456789abcdef0123456789abcdef01234567";
    const TARGET_FOLDER_ID: &str = "42";

    #[test]
    fn mock_upload_parse_submit_and_status_query_follow_one_torrent_contract() {
        let mut api = Mock115 {
            calls: Vec::new(),
            upload_reply: UploadReply::Uploaded,
            reply: MockReply::Accepted,
        };

        let outcome = run_single_torrent(
            &mut api,
            TORRENT_BYTES,
            TORRENT_SHA1,
            INFO_HASH,
            TARGET_FOLDER_ID,
            &[0],
        )
        .expect("mock contract flow");

        assert!(matches!(
            outcome,
            ContractOutcome::RemoteTaskFound { status: 1 }
        ));
        assert_eq!(
            api.calls,
            vec![
                MockCall::Upload {
                    bytes: TORRENT_BYTES.to_vec(),
                    torrent_sha1: TORRENT_SHA1.to_string(),
                    target_folder_id: TARGET_FOLDER_ID.to_string(),
                    endpoints: vec![
                        "GET /open/upload/get_token",
                        "POST /open/upload/init",
                        "OSS object upload with callback",
                    ],
                },
                MockCall::Parse {
                    torrent_sha1: TORRENT_SHA1.to_string(),
                    pick_code: "mock-pick-code".to_string(),
                    endpoint: "POST /open/offline/torrent",
                },
                MockCall::Submit {
                    request: AddTaskBtRequest {
                        info_hash: INFO_HASH.to_string(),
                        wanted: "0".to_string(),
                        save_path: String::new(),
                        torrent_sha1: TORRENT_SHA1.to_string(),
                        pick_code: "mock-pick-code".to_string(),
                        wp_path_id: TARGET_FOLDER_ID.to_string(),
                    },
                    endpoint: "POST /open/offline/add_task_bt",
                },
                MockCall::ListTasks {
                    page: 1,
                    endpoint: "GET /open/offline/get_task_list",
                },
            ]
        );
    }

    #[test]
    fn unknown_submit_is_checked_remotely_once_without_resubmission() {
        let mut api = Mock115 {
            calls: Vec::new(),
            upload_reply: UploadReply::Uploaded,
            reply: MockReply::Unknown,
        };

        let outcome = run_single_torrent(
            &mut api,
            TORRENT_BYTES,
            TORRENT_SHA1,
            INFO_HASH,
            TARGET_FOLDER_ID,
            &[0],
        )
        .expect("unknown submit remains recoverable");

        assert_eq!(outcome, ContractOutcome::RemoteStateUnknown);
        assert_eq!(
            api.calls
                .iter()
                .filter(|call| matches!(call, MockCall::Submit { .. }))
                .count(),
            1
        );
        assert!(matches!(
            api.calls.last(),
            Some(MockCall::ListTasks { page: 1, .. })
        ));
    }

    #[test]
    fn wanted_indexes_are_validated_before_the_single_write() {
        let mut api = Mock115 {
            calls: Vec::new(),
            upload_reply: UploadReply::Uploaded,
            reply: MockReply::Accepted,
        };

        let result = run_single_torrent(
            &mut api,
            TORRENT_BYTES,
            TORRENT_SHA1,
            INFO_HASH,
            TARGET_FOLDER_ID,
            &[1],
        );

        assert!(result.is_err());
        assert!(!api
            .calls
            .iter()
            .any(|call| matches!(call, MockCall::Submit { .. })));
    }

    #[test]
    fn unknown_upload_stops_before_parse_or_submission() {
        let mut api = Mock115 {
            calls: Vec::new(),
            upload_reply: UploadReply::Unknown,
            reply: MockReply::Accepted,
        };

        let outcome = run_single_torrent(
            &mut api,
            TORRENT_BYTES,
            TORRENT_SHA1,
            INFO_HASH,
            TARGET_FOLDER_ID,
            &[0],
        )
        .expect("unknown upload remains unresolved");

        assert_eq!(outcome, ContractOutcome::RemoteStateUnknown);
        assert_eq!(api.calls.len(), 1);
        assert!(matches!(api.calls[0], MockCall::Upload { .. }));
    }
}
