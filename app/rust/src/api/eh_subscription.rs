//! EH 订阅（可选插件）的桥接层。
//!
//! 边界：只读写「规则 JSON + 保存目录 + manifest.json」，不触碰书源/目录库/阅读数据。
//! 所有阻塞 HTTP 都经 `spawn_blocking` 隔离，避免卡住 Flutter 主线程。

use crate::eh_subscription as eh;

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
        if let Ok(text) = std::fs::read_to_string(&path) {
            // 校验一次：损坏的 JSON 不应静默通过
            eh::EhRules::from_json(&text)?;
            Ok(text)
        } else {
            let default = eh::EhRules::default();
            let text = default.to_json();
            if let Some(dir) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            std::fs::write(&path, &text).map_err(|e| format!("写入默认规则失败：{e}"))?;
            Ok(text)
        }
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
}

/// 保存规则 JSON（先校验再落盘，避免写出损坏配置）。
pub async fn eh_save_rules(path: String, rules_json: String) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let rules = eh::EhRules::from_json(&rules_json)?;
        if let Some(dir) = std::path::Path::new(&path).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&path, rules.to_json()).map_err(|e| format!("保存规则失败：{e}"))
    })
    .await
    .map_err(|e| format!("任务失败：{e}"))?
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
pub async fn eh_collect(rules_json: String) -> Result<String, String> {
    let rules = rules_from_arg(rules_json)?;
    tokio::task::spawn_blocking(move || {
        let progress = eh::collect(&rules, false)?;
        serde_json::to_string(&progress).map_err(|e| format!("进度序列化失败：{e}"))
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
        let snapshot: crate::eh_import::BookSnapshot = serde_json::from_str(&snapshot_json)
            .map_err(|e| format!("snapshot 解析失败：{e}"))?;
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
