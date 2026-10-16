//! EH 订阅（可选插件）的桥接层。
//!
//! 边界：只读写「规则 JSON + 保存目录 + manifest.json」，不触碰书源/目录库/阅读数据。
//! 所有阻塞 HTTP 都经 `spawn_blocking` 隔离，避免卡住 Flutter 主线程。

use crate::eh_subscription as eh;
use std::io::Write as _;
use std::path::Path;

fn rules_from_arg(rules_json: String) -> Result<eh::EhRules, String> {
    eh::EhRules::from_json(&rules_json)
}

/// 默认规则 JSON（首次打开订阅面板时使用）。
pub fn eh_default_rules() -> String {
    eh::EhRules::default().to_json()
}

/// 读取规则文件；文件不存在时回写默认规则并返回默认值。
pub async fn eh_load_rules(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        eh::with_rules_config_lock(Path::new(&path), || {
            load_rules_text_unlocked(Path::new(&path))
        })
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

fn existing_rules_text(result: std::io::Result<String>) -> Result<Option<String>, String> {
    match result {
        Ok(text) => {
            eh::EhRules::from_json(&text)?;
            Ok(Some(text))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("读取规则失败：{error}")),
    }
}

fn load_rules_text_unlocked(path: &Path) -> Result<String, String> {
    if let Some(text) = existing_rules_text(std::fs::read_to_string(path))? {
        return Ok(text);
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建规则目录失败：{e}"))?;
    }
    let text = eh::EhRules::default().to_json();
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return existing_rules_text(std::fs::read_to_string(path))?
                .ok_or_else(|| "规则文件在初始化期间消失".to_string());
        }
        Err(error) => return Err(format!("写入默认规则失败：{error}")),
    };
    file.write_all(text.as_bytes())
        .and_then(|()| file.flush())
        .and_then(|()| file.sync_all())
        .map_err(|e| format!("写入默认规则失败：{e}"))?;
    Ok(text)
}

/// Save only fields changed since base_json; current Daily state is merged under
/// the same config-scoped transaction lock.
pub async fn eh_save_rules(
    path: String,
    base_json: String,
    rules_json: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        eh::save_rules_merged(Path::new(&path), &base_json, &rules_json)
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// Persist only the explicit Daily switch change against the latest config.
pub async fn eh_set_daily_enabled(path: String, enabled: bool) -> Result<String, String> {
    tokio::task::spawn_blocking(move || eh::set_daily_enabled(Path::new(&path), enabled))
        .await
        .map_err(|e| format!("任务失败：{e}"))?
}

/// Atomically claim one local date. The response contains the durable config
/// snapshot and whether this process won the claim.
pub async fn eh_claim_daily(path: String, date: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let (claimed, rules_json) = eh::claim_daily(Path::new(&path), &date)?;
        serde_json::to_string(&serde_json::json!({
            "claimed": claimed,
            "rules_json": rules_json,
        }))
        .map_err(|error| format!("EH Daily 领取结果序列化失败：{error}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// Persist a checked state transition without replacing the enable flag or
/// the last claimed date.
pub async fn eh_update_daily_run(path: String, run_json: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let run: eh::EhAutomationRun = serde_json::from_str(&run_json)
            .map_err(|error| format!("EH Daily 运行状态 JSON 无效：{error}"))?;
        eh::update_daily_run(Path::new(&path), run)
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// Mark stale work interrupted only after checking the Windows collection
/// lease; a second live RCH process must not be mistaken for a crash.
pub async fn eh_recover_daily_run(path: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || eh::recover_daily_run(Path::new(&path)))
        .await
        .map_err(|e| format!("任务失败：{e}"))?
}

#[cfg(test)]
mod tests {
    use super::{
        eh_claim_daily, eh_load_rules, eh_save_rules, eh_update_daily_run, existing_rules_text,
    };
    use crate::eh_subscription::{EhAutomationRun, EhRules};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn test_rules_path() -> std::path::PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "rch-eh-daily-api-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).expect("create isolated rules directory");
        root.join("eh_subscription_rules.json")
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("build test runtime")
    }

    #[test]
    fn daily_claim_is_atomic_persisted_and_preserved_by_stale_rule_edits() {
        let path = test_rules_path();
        let runtime = runtime();

        runtime.block_on(async {
            let defaults = eh_load_rules(path.to_string_lossy().into_owned())
                .await
                .expect("load isolated defaults");
            let mut enabled = EhRules::from_json(&defaults).expect("parse defaults");
            enabled.automation.daily_enabled = true;
            std::fs::write(&path, enabled.to_json()).expect("enable Daily in fixture");

            let first: serde_json::Value = serde_json::from_str(
                &eh_claim_daily(path.to_string_lossy().into_owned(), "2026-10-11".into())
                    .await
                    .expect("claim today's run"),
            )
            .expect("parse claim result");
            assert_eq!(
                first["claimed"], true,
                "the first process should claim the date"
            );
            let duplicate: serde_json::Value = serde_json::from_str(
                &eh_claim_daily(path.to_string_lossy().into_owned(), "2026-10-11".into())
                    .await
                    .expect("check duplicate claim"),
            )
            .expect("parse duplicate claim result");
            assert_eq!(
                duplicate["claimed"], false,
                "a date may only be claimed once"
            );

            let mut stale_rules = EhRules::from_json(&defaults).expect("parse stale rules");
            stale_rules.search = "updated query".into();
            let saved = eh_save_rules(
                path.to_string_lossy().into_owned(),
                defaults.clone(),
                stale_rules.to_json(),
            )
            .await
            .expect("save a stale rule edit without rewinding Daily state");
            let stored = EhRules::from_json(&saved).expect("parse saved merged rules");
            assert_eq!(stored.search, "updated query");
            assert!(stored.automation.daily_enabled);
            assert_eq!(
                stored.automation.last_auto_attempt_date.as_deref(),
                Some("2026-10-11")
            );
            assert_eq!(
                stored
                    .automation
                    .last_run
                    .as_ref()
                    .map(|run| run.status.as_str()),
                Some("claimed")
            );

            let running = EhAutomationRun {
                date: "2026-10-11".into(),
                source: "daily_auto".into(),
                status: "running".into(),
                ..Default::default()
            };
            eh_update_daily_run(
                path.to_string_lossy().into_owned(),
                serde_json::to_string(&running).expect("serialize running state"),
            )
            .await
            .expect("persist running state");

            let mut independent_edit = EhRules::from_json(&defaults).expect("parse old base");
            independent_edit.title_markers = "anthology".into();
            let saved = eh_save_rules(
                path.to_string_lossy().into_owned(),
                defaults,
                independent_edit.to_json(),
            )
            .await
            .expect("merge a different stale rule field");
            let stored = EhRules::from_json(&saved).expect("parse merged rules");
            assert_eq!(stored.search, "updated query");
            assert_eq!(stored.title_markers, "anthology");
            assert_eq!(
                stored
                    .automation
                    .last_run
                    .as_ref()
                    .map(|run| run.status.as_str()),
                Some("running")
            );
        });

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture parent"));
    }

    #[test]
    fn disabling_daily_after_claim_prevents_collection_start() {
        let path = test_rules_path();
        let runtime = runtime();

        runtime.block_on(async {
            let defaults = eh_load_rules(path.to_string_lossy().into_owned())
                .await
                .expect("load isolated defaults");
            let mut enabled = EhRules::from_json(&defaults).expect("parse defaults");
            enabled.automation.daily_enabled = true;
            std::fs::write(&path, enabled.to_json()).expect("enable Daily in fixture");
            assert!(
                crate::eh_subscription::claim_daily(&path, "2026-10-12")
                    .unwrap()
                    .0
            );
            eh_update_daily_run(
                path.to_string_lossy().into_owned(),
                serde_json::to_string(&EhAutomationRun {
                    date: "2026-10-12".into(),
                    source: "daily_auto".into(),
                    status: "running".into(),
                    ..Default::default()
                })
                .unwrap(),
            )
            .await
            .expect("persist claimed running state");
            super::eh_set_daily_enabled(path.to_string_lossy().into_owned(), false)
                .await
                .expect("disable before network work");

            let error = crate::eh_subscription::validate_daily_collection_start(
                &path,
                "2026-10-12",
                "daily_auto",
            )
            .expect_err("a disabled Daily task must not enter EH collection");
            assert!(error.contains("Daily 已关闭"));
        });

        let _ = std::fs::remove_dir_all(path.parent().expect("fixture parent"));
    }

    #[test]
    fn unreadable_existing_rules_are_not_treated_as_missing_defaults() {
        let error = existing_rules_text(Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected read denial",
        )))
        .expect_err("an unreadable existing rules file must stop loading");

        assert!(error.contains("读取规则失败"));
    }

    #[test]
    fn missing_rules_are_the_only_case_that_requests_defaults() {
        assert!(existing_rules_text(Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "missing",
        )))
        .unwrap()
        .is_none());
    }
}

/// 当前扫描进度（UI 轮询；JSON 编码避免为进度单独生成 DTO）。
pub fn eh_progress() -> String {
    serde_json::to_string(&eh::progress()).unwrap_or_else(|_| "{}".into())
}

/// 请求取消当前扫描（在下一个候选边界生效）。
pub fn eh_cancel() {
    eh::request_cancel();
}

/// 连通性预检：主站与 tracker 分别是否可达（国内网络下主站常需代理）。
pub async fn eh_probe(rules_json: String) -> Result<String, String> {
    let rules = rules_from_arg(rules_json)?;
    tokio::task::spawn_blocking(move || {
        let probe = eh::probe_connectivity(&rules);
        serde_json::to_string(&probe).map_err(|e| format!("预检结果序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// 执行一轮扫描。返回的是结束后的一次进度快照（UI 亦可轮询 `eh_progress`）。
pub async fn eh_collect(
    path: String,
    rules_json: String,
    source: String,
    date: String,
) -> Result<String, String> {
    let rules = rules_from_arg(rules_json)?;
    tokio::task::spawn_blocking(move || {
        let _collection_lock = eh::try_acquire_collection_lock(Path::new(&path))?
            .ok_or_else(|| "同一 EH 配置已有另一个 RCH 进程在收集".to_string())?;
        eh::validate_daily_collection_start(Path::new(&path), &date, &source)?;
        let progress = eh::collect(&rules, false)?;
        serde_json::to_string(&progress).map_err(|e| format!("进度序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// **实时搜索**规划一本书的导入（影子模式，不写库）。
///
/// 与 `eh_plan_import`（读 manifest、离线）不同：候选来自**实时搜索 E 站**
/// （锚点依次尝试 → gdata → Dice 判定），因此不受"manifest 只含订阅命中项"的限制，
/// 这是修复"大多数书识别不出来"的关键。
/// 命中后会带回画廊语义层（标签/作者/系列/语言…），缺失译名时按需联网更新。
pub async fn eh_plan_book_live(
    rules_json: String,
    work_title: String,
    creators_json: String,
    snapshot_json: String,
    number: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let rules = rules_from_arg(rules_json)?;
        let creators: Vec<String> =
            serde_json::from_str(&creators_json).map_err(|e| format!("creators 解析失败：{e}"))?;
        let snapshot: crate::eh_import::BookSnapshot =
            serde_json::from_str(&snapshot_json).map_err(|e| format!("snapshot 解析失败：{e}"))?;
        let (decision, semantic, anchor_kind) =
            eh::match_gallery_full(&rules, &work_title, &creators, &number)?;
        let mut plan = crate::eh_import::plan_import(&snapshot, &semantic, &decision);
        // 命中依据必须如实：创作者兜底 ≠ 作品名命中（后者才允许自动写入）
        if anchor_kind == "creator" {
            plan.matched_by = "creator".into();
        }
        serde_json::to_string(&plan).map_err(|e| format!("计划序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// 影子模式：从已落盘的 manifest 规划"将要导入什么"（**不写库**）。
///
/// 候选直接取自 manifest 的语义层，因此**离线可复现**（不联网搜索）。
/// `creators_json` 为本地作品名之外的兜底锚点（JSON 字符串数组）；
/// `snapshot_json` 为本地现状（`eh_import::BookSnapshot`）。
pub async fn eh_plan_import(
    manifest_dir: String,
    work_title: String,
    creators_json: String,
    snapshot_json: String,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let creators: Vec<String> =
            serde_json::from_str(&creators_json).map_err(|e| format!("creators 解析失败：{e}"))?;
        let snapshot: crate::eh_import::BookSnapshot =
            serde_json::from_str(&snapshot_json).map_err(|e| format!("snapshot 解析失败：{e}"))?;
        let items = eh::read_manifest(&manifest_dir);
        let plan = crate::eh_import::plan_from_manifest(&items, &snapshot, &work_title, &creators);
        serde_json::to_string(&plan).map_err(|e| format!("计划序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// 读取已保存清单（文件不存在返回空列表）。
pub async fn eh_manifest(out_dir: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let items = eh::read_manifest(&out_dir);
        serde_json::to_string(&items).map_err(|e| format!("清单序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// Local, non-destructive manifest and referenced-Torrent check for the handoff UI.
pub async fn eh_manifest_checked(out_dir: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let report = eh::read_manifest_checked(Path::new(&out_dir));
        serde_json::to_string(&report).map_err(|e| format!("清单校验结果序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// Enumerate only immediate `.torrent` files in the configured output folder.
/// This local read is independent of `manifest.json` and never recurses.
pub async fn eh_torrents_checked(out_dir: String) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let report = eh::read_torrent_directory_checked(Path::new(&out_dir));
        serde_json::to_string(&report).map_err(|e| format!("Torrent 目录校验结果序列化失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}
