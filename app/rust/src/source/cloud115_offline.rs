use super::{Cloud115WebClient, WebFilesResp, WEB_API_FILES, WEB_PAGE_SIZE};
use aes::Aes128;
use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use cbc::cipher::{block_padding::Pkcs7, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use md5::{Digest as Md5Digest, Md5};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

const BT_TORRENT_MAX_BYTES: u64 = 32 * 1024 * 1024;
const BT_FOLDER_PAGE_BUDGET: i64 = 10;
const BT_TASK_PAGE_BUDGET: u64 = 10;
const BT_TASK_PAGE_SIZE: u64 = 30;
const BT_APP_VERSION: &str = "36.2.28";
const BT_SSP_USER_AGENT: &str =
    "Mozilla/5.0 115disk/36.2.28 115Browser/36.2.28 115wangpan_android/36.2.28";
const UPLOAD_USER_AGENT: &str =
    "Mozilla/5.0 115disk/36.2.28 115Browser/36.2.28 115wangpan_android/36.2.28";
const UPLOAD_AES_KEY: [u8; 16] = [
    0xfb, 0x1a, 0x19, 0xd6, 0x52, 0xf5, 0xaa, 0xf7, 0xbc, 0x65, 0x1d, 0x0f, 0x69, 0xbf, 0x42, 0x2f,
];
const UPLOAD_AES_IV: [u8; 16] = [
    0x69, 0xbf, 0x42, 0x2f, 0x49, 0x96, 0x05, 0x50, 0xa0, 0xad, 0x44, 0xec, 0x34, 0x46, 0xcb, 0x4c,
];
const UPLOAD_ECDH_PUBKEY: [u8; 30] = [
    0x1d, 0x03, 0x0e, 0x80, 0xa1, 0x78, 0xdc, 0xee, 0xce, 0xcd, 0xa3, 0x77, 0xde, 0x12, 0x8d, 0x8e,
    0xd9, 0xdd, 0xcf, 0x55, 0xae, 0x61, 0xed, 0x46, 0xea, 0x12, 0x1a, 0x1c, 0xfc, 0x81,
];
const UPLOAD_CRC_SALT: &[u8] = b"^j>WD3Kr?J2gLFjD4W2y@";
const UPLOAD_MD5_SALT: &[u8] = b"Qclm8MGWUv59TnrR0XPg";
const UPLOAD_RESPONSE_MAX_BYTES: usize = 2 * 1024 * 1024;
const UPLOAD_RESPONSE_MAX_DECOMPRESSED: usize = 8 * 1024 * 1024;
const UPLOAD_RESPONSE_BLOCK_SIZE: usize = 0x2000;

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtFile {
    pub index: u32,
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtFolder {
    pub id: String,
    pub name: String,
}

/// A directory chosen by the user in the read-only folder picker. The backend
/// rechecks the final folder against its parent before every BT operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cloud115BtFolderSelection {
    pub id: String,
    pub parent_id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtPreflight {
    pub account_id: String,
    pub target_folder_path: String,
    pub target_folder_id: String,
    pub torrent_name: String,
    pub torrent_size: u64,
    pub torrent_sha1: String,
    pub info_hash: String,
    pub files: Vec<Cloud115BtFile>,
    /// `found`, `not_found`, or `unknown`. Unknown blocks any future submit.
    pub duplicate_status: String,
    /// Safe local explanation when the task list cannot prove absence.
    pub duplicate_detail: String,
    pub write_gate: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtUploadParseResult {
    pub state: String,
    pub message: String,
    pub account_id: String,
    pub target_folder_path: String,
    pub target_folder_id: String,
    pub torrent_name: String,
    pub torrent_size: u64,
    pub torrent_sha1: String,
    pub info_hash: String,
    pub pick_code: Option<String>,
    pub upload_reused: bool,
    pub files: Vec<Cloud115BtFile>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtTaskStatus {
    pub state: String,
    pub info_hash: String,
    pub status_code: Option<i64>,
    pub status_text: String,
    pub percent_done: Option<f64>,
    pub target_folder_id: String,
}

/// One task row from a complete, bounded account task-list snapshot. The
/// provider's status code is retained as evidence; undocumented values are not
/// translated into terminal states.
#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtTaskSnapshotItem {
    pub info_hash: String,
    /// Destination parent from the remote task; None means it was not proven.
    pub target_folder_id: Option<String>,
    pub status_code: Option<i64>,
    pub percent_done: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtTaskSnapshot {
    pub account_id: String,
    pub state: String,
    pub complete: bool,
    pub pages_read: u32,
    pub error_class: Option<String>,
    pub message: String,
    pub tasks: Vec<Cloud115BtTaskSnapshotItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Cloud115BtSubmitResult {
    pub state: String,
    pub info_hash: String,
    pub message: String,
    pub target_folder_id: String,
}

enum BtUploadParseOutcome {
    Parsed(Vec<Cloud115BtFile>),
    Blocked(&'static str, &'static str),
}

#[derive(Debug, Clone)]
pub(super) struct BtEndpoints {
    pub(super) files_list: String,
    pub(super) upload_info: String,
    pub(super) upload_init: String,
    pub(super) upload_token: String,
    pub(super) upload_object_base: String,
    pub(super) task_list: String,
    pub(super) torrent_info: String,
    pub(super) submit_bt: String,
}

impl BtEndpoints {
    pub(super) fn production() -> Self {
        Self {
            files_list: WEB_API_FILES.into(),
            upload_info: "https://webapi.115.com/files/uploadinfo".into(),
            upload_init: "https://uplb.115.com/4.0/initupload.php".into(),
            upload_token: "https://uplb.115.com/3.0/gettoken.php".into(),
            upload_object_base: "https://fhnfile.oss-cn-shenzhen.aliyuncs.com/".into(),
            task_list: "https://clouddownload.115.com/web/".into(),
            torrent_info: "https://clouddownload.115.com/".into(),
            submit_bt: "https://clouddownload.115.com/lixianssp/".into(),
        }
    }
}

struct UploadInitInput<'a> {
    user_id: &'a str,
    user_key: &'a str,
    file_sha1: &'a str,
    filename: &'a str,
    file_size: u64,
    target_folder_id: &'a str,
    sign_key: &'a str,
    sign_val: &'a str,
    timestamp: u64,
}

struct UploadInitPacket {
    k_ec: String,
    body: Vec<u8>,
}

struct ParsedTorrent {
    info_hash: String,
    torrent_sha1: String,
    files: Vec<Cloud115BtFile>,
}

struct UploadIdentity {
    user_id: String,
    user_key: String,
}

struct UploadInitResult {
    status: u64,
    pick_code: String,
    bucket: String,
    object: String,
    callback: String,
    callback_var: String,
    sign_key: String,
    sign_check: String,
}

struct OssToken {
    access_key_id: String,
    access_key_secret: String,
    security_token: String,
}

fn parse_upload_identity(
    response: &serde_json::Value,
    cookie_user_id: &str,
) -> Result<UploadIdentity> {
    if response.get("state").and_then(serde_json::Value::as_bool) != Some(true) {
        bail!("115 上传身份接口未返回成功状态");
    }
    let data = response.get("data").unwrap_or(response);
    let user_id = data
        .get("user_id")
        .or_else(|| data.get("userid"))
        .and_then(value_as_string)
        .ok_or_else(|| anyhow!("115 上传身份响应缺少用户标识"))?;
    let user_key = data
        .get("userkey")
        .or_else(|| data.get("user_key"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty() && value.is_ascii())
        .ok_or_else(|| anyhow!("115 上传身份响应缺少有效用户密钥"))?;
    if user_id != cookie_user_id || !user_id.bytes().all(|byte| byte.is_ascii_digit()) {
        bail!("115 上传身份与当前 Cookie 会话账号不一致");
    }
    Ok(UploadIdentity {
        user_id,
        user_key: user_key.to_string(),
    })
}

fn parse_upload_init_result(response: &serde_json::Value) -> Result<UploadInitResult> {
    if response.get("state").and_then(serde_json::Value::as_bool) == Some(false) {
        let code = response
            .get("errno")
            .or_else(|| response.get("code"))
            .and_then(value_as_string)
            .unwrap_or_else(|| "unknown".to_string());
        bail!("115 上传初始化被拒绝（错误码 {code}）");
    }
    let data = response
        .get("data")
        .ok_or_else(|| anyhow!("115 上传初始化响应缺少 data"))?;
    let status = data
        .get("status")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("115 上传初始化响应缺少 status"))?;
    let pick_code = response
        .get("pickcode")
        .or_else(|| response.get("pick_code"))
        .or_else(|| data.get("pickcode"))
        .or_else(|| data.get("pick_code"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if matches!(status, 1 | 2) && pick_code.trim().is_empty() {
        bail!("115 上传初始化未返回 pickcode");
    }
    let bucket = data
        .get("bucket")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("fhnfile")
        .to_string();
    let object = data
        .get("object")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let callback = data
        .get("callback")
        .and_then(|value| value.get("callback"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let callback_var = data
        .get("callback")
        .and_then(|value| value.get("callback_var"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sign_key = data
        .get("sign_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let sign_check = data
        .get("sign_check")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    if status == 7 && (sign_key.is_empty() || sign_check.is_empty()) {
        bail!("115 上传二次校验响应缺少范围字段");
    }
    if status == 1
        && (bucket != "fhnfile"
            || object.is_empty()
            || callback.is_empty()
            || callback_var.is_empty())
    {
        bail!("115 上传初始化返回了不支持的 OSS 配置");
    }
    Ok(UploadInitResult {
        status,
        pick_code,
        bucket,
        object,
        callback,
        callback_var,
        sign_key,
        sign_check,
    })
}

fn parse_upload_token(response: &serde_json::Value) -> Result<OssToken> {
    if response
        .get("StatusCode")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| status != "200")
    {
        bail!("115 OSS 上传凭证请求被拒绝");
    }
    let value = response.get("data").unwrap_or(response);
    let access_key_id = value
        .get("AccessKeyId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("115 OSS 上传凭证缺少 AccessKeyId"))?;
    let access_key_secret = value
        .get("AccessKeySecret")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("115 OSS 上传凭证缺少 AccessKeySecret"))?;
    let security_token = value
        .get("SecurityToken")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("115 OSS 上传凭证缺少 SecurityToken"))?;
    Ok(OssToken {
        access_key_id: access_key_id.to_string(),
        access_key_secret: access_key_secret.to_string(),
        security_token: security_token.to_string(),
    })
}

fn parse_remote_torrent_parse(
    response: &serde_json::Value,
    expected_info_hash: &str,
    expected_files: &[Cloud115BtFile],
) -> Result<Vec<Cloud115BtFile>> {
    if response.get("state").and_then(serde_json::Value::as_bool) != Some(true) {
        bail!("115 Torrent 解析接口未返回成功状态");
    }
    let data = response.get("data").unwrap_or(response);
    let info_hash = data
        .get("info_hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow!("115 Torrent 解析响应缺少 info_hash"))?;
    if !info_hash.eq_ignore_ascii_case(expected_info_hash) {
        bail!("115 解析出的 infohash 与本地 Torrent 不一致");
    }
    let entries = data
        .get("torrent_filelist_web")
        .or_else(|| data.get("files"))
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("115 Torrent 解析响应缺少文件清单"))?;
    if entries.len() != expected_files.len() {
        bail!("115 Torrent 文件数与本地 bencode 不一致");
    }
    let mut files = Vec::with_capacity(entries.len());
    for (index, (entry, expected)) in entries.iter().zip(expected_files).enumerate() {
        let path = entry
            .get("path")
            .or_else(|| entry.get("name"))
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow!("115 Torrent 解析文件项缺少路径"))?;
        let size = entry
            .get("size")
            .and_then(value_as_u64)
            .ok_or_else(|| anyhow!("115 Torrent 解析文件项缺少大小"))?;
        if path != expected.path || size != expected.size || expected.index as usize != index {
            bail!("115 Torrent 解析文件清单与本地 bencode 不一致");
        }
        files.push(Cloud115BtFile {
            index: index as u32,
            path: path.to_string(),
            size,
        });
    }
    Ok(files)
}

fn value_as_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_u64().map(|number| number.to_string()))
}

fn value_as_u64(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
}

fn parse_sign_check_range(value: &str, file_size: usize) -> Result<std::ops::Range<usize>> {
    let (start, end) = value
        .split_once('-')
        .ok_or_else(|| anyhow!("115 上传二次校验范围格式无效"))?;
    let start = start.parse::<usize>().context("115 上传二次校验起点无效")?;
    let end = end.parse::<usize>().context("115 上传二次校验终点无效")?;
    if start > end || end >= file_size || end - start >= 1024 * 1024 {
        bail!("115 上传二次校验范围超出 Torrent 文件边界");
    }
    Ok(start..end + 1)
}

#[cfg(test)]
fn build_oss_upload_url(bucket: &str, object: &str) -> Result<String> {
    build_oss_upload_url_at(
        "https://fhnfile.oss-cn-shenzhen.aliyuncs.com/",
        bucket,
        object,
        false,
    )
}

fn build_oss_upload_url_at(
    base: &str,
    bucket: &str,
    object: &str,
    allow_loopback_http: bool,
) -> Result<String> {
    let object = object.trim_start_matches('/');
    if bucket != "fhnfile"
        || object.is_empty()
        || object
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !object
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~/".contains(&byte))
    {
        bail!("115 OSS 返回了不安全或不支持的目标地址");
    }
    let base_url = reqwest::Url::parse(base).context("115 OSS 基础地址无效")?;
    let production_host = base_url.host_str() == Some("fhnfile.oss-cn-shenzhen.aliyuncs.com");
    let loopback_test = allow_loopback_http
        && base_url.scheme() == "http"
        && base_url
            .host_str()
            .is_some_and(|host| host == "127.0.0.1" || host == "[::1]" || host == "localhost");
    if !((base_url.scheme() == "https" && production_host) || loopback_test)
        || base_url.username() != ""
        || base_url.password().is_some()
        || base_url.query().is_some()
        || base_url.fragment().is_some()
    {
        bail!("115 OSS 地址不是受信任的 HTTPS 端点");
    }
    let base_url = if base_url.path().ends_with('/') {
        base_url
    } else {
        reqwest::Url::parse(&format!("{base}/"))?
    };
    Ok(base_url.join(object)?.to_string())
}

fn build_oss_authorization(
    token: &OssToken,
    method: &str,
    bucket: &str,
    path: &str,
    date: &str,
    content_type: &str,
    callback: &str,
    callback_var: &str,
) -> Result<String> {
    use ring::hmac;

    let headers = BTreeMap::from([
        (
            "x-oss-callback".to_string(),
            base64::engine::general_purpose::STANDARD.encode(callback),
        ),
        (
            "x-oss-callback-var".to_string(),
            base64::engine::general_purpose::STANDARD.encode(callback_var),
        ),
        (
            "x-oss-security-token".to_string(),
            token.security_token.clone(),
        ),
    ]);
    let canonical_headers = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}"))
        .collect::<Vec<_>>()
        .join("\n");
    let canonical = format!(
        "{}\n\n{}\n{}\n{}\n/{}{}",
        method.to_ascii_uppercase(),
        content_type,
        date,
        canonical_headers,
        bucket,
        path
    );
    let key = hmac::Key::new(
        hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY,
        token.access_key_secret.as_bytes(),
    );
    let signature = base64::engine::general_purpose::STANDARD
        .encode(hmac::sign(&key, canonical.as_bytes()).as_ref());
    Ok(format!("OSS {}:{signature}", token.access_key_id))
}

fn ensure_bt_read_status(_client: &Cloud115WebClient, status: u16, operation: &str) -> Result<()> {
    if let Some((_, message)) = bt_gate_status_result(status) {
        bail!("{operation}：{message}");
    }
    if !(200..300).contains(&status) {
        bail!("{operation} 失败（HTTP {status}）");
    }
    Ok(())
}

fn bt_gate_status_result(status: u16) -> Option<(&'static str, &'static str)> {
    match status {
        405 => Some((
            "WAF_BLOCKED",
            "115 返回 WAF 405；已停止该 Cookie 会话后台请求",
        )),
        429 => Some(("RATE_LIMITED", "115 返回 HTTP 429 限流；没有重试")),
        401 => Some(("AUTH_EXPIRED", "115 Cookie 会话鉴权失败（HTTP 401）")),
        403 => Some(("ACCESS_DENIED", "115 拒绝当前 Cookie 会话访问（HTTP 403）")),
        _ => None,
    }
}

fn unknown_task_status(info_hash: &str, reason: &str) -> Cloud115BtTaskStatus {
    Cloud115BtTaskStatus {
        state: "unknown".into(),
        info_hash: info_hash.to_ascii_lowercase(),
        status_code: None,
        status_text: reason.to_string(),
        percent_done: None,
        target_folder_id: String::new(),
    }
}

fn not_found_task_status(info_hash: &str) -> Cloud115BtTaskStatus {
    Cloud115BtTaskStatus {
        state: "not_found".into(),
        info_hash: info_hash.to_ascii_lowercase(),
        status_code: None,
        status_text: String::new(),
        percent_done: None,
        target_folder_id: String::new(),
    }
}

fn bt_operation_key(account_id: &str, info_hash: &str, target_folder_id: &str) -> String {
    format!(
        "{}:{}:{}",
        account_id,
        info_hash.to_ascii_lowercase(),
        target_folder_id
    )
}

fn parse_ssp_response(body: &[u8]) -> Result<serde_json::Value> {
    let mut response: serde_json::Value = serde_json::from_slice(body)
        .or_else(|_| decode_upload_init_response(body))
        .context("解析 115 SSP 响应失败")?;
    if let Some(encoded) = response.get("data").and_then(serde_json::Value::as_str) {
        if let Ok(decoded) = super::m115_decode(encoded) {
            if let Ok(data) = serde_json::from_slice::<serde_json::Value>(&decoded) {
                response["data"] = data;
            }
        }
    }
    Ok(response)
}

fn bt_rejection_message(response: &serde_json::Value) -> String {
    let code = ["errno", "errcode", "code"]
        .iter()
        .filter_map(|key| response.get(key))
        .find_map(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str()?.parse::<i64>().ok())
        })
        .filter(|code| *code != 0);
    match code {
        Some(code) => format!("115 明确拒绝 BT 提交（错误码 {code}）；本次不会重试。"),
        None => "115 明确拒绝 BT 提交；本次不会重试。".into(),
    }
}

#[derive(Clone, Debug)]
struct FolderCandidate {
    name: String,
    id: String,
    is_dir: bool,
}

impl Cloud115WebClient {
    /// Lists one bounded level of folders for the user-invoked destination
    /// picker. It reuses this reader session and never records open time.
    pub fn bt_list_child_folders(&self, parent_id: &str) -> Result<Vec<Cloud115BtFolder>> {
        self.ensure_web_request_allowed()?;
        let folders = self.read_folder_children(parent_id)?;
        Ok(folders
            .into_iter()
            .map(|folder| Cloud115BtFolder {
                id: folder.id,
                name: folder.name,
            })
            .collect())
    }

    /// Validates exactly one H0-manifest-relative Torrent and performs bounded,
    /// read-only account/folder checks on the existing Cookie session. Remote
    /// duplicate handling is left to 115 when the user confirms submission.
    pub fn bt_readonly_preflight(
        &self,
        output_dir: &str,
        relative_file: &str,
        expected_info_hash: &str,
        target: &Cloud115BtFolderSelection,
    ) -> Result<Cloud115BtPreflight> {
        self.ensure_web_request_allowed()?;
        let parsed = load_checked_torrent(output_dir, relative_file, expected_info_hash)?;
        let cookie = self.current_cookie();
        let account_id = super::user_id_from_cookie(&cookie)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("无法从当前 115 Cookie 会话确认账号标识"))?;
        let target_folder_id = self.validate_target_folder_selection(target)?;
        let duplicate_status = "submit_first".to_string();
        let duplicate_detail = String::new();
        let torrent_name = Path::new(relative_file)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("Torrent 文件名不是有效 UTF-8"))?
            .to_string();
        let torrent_size =
            std::fs::metadata(canonical_manifest_file(output_dir, relative_file)?)?.len();
        Ok(Cloud115BtPreflight {
            account_id,
            target_folder_path: target.path.clone(),
            target_folder_id,
            torrent_name,
            torrent_size,
            torrent_sha1: parsed.torrent_sha1,
            info_hash: parsed.info_hash,
            files: parsed.files,
            duplicate_status,
            duplicate_detail,
            write_gate: "REQUIRES_EXPLICIT_CONFIRMATION".to_string(),
        })
    }

    /// Deletes exactly one H0-manifest-relative Torrent only after this same
    /// Cookie session confirms the account and a positive remote task match.
    /// The manifest and all other files remain untouched.
    pub fn bt_delete_local_torrent_if_remote_task_found(
        &self,
        output_dir: &str,
        relative_file: &str,
        expected_info_hash: &str,
        expected_account_id: &str,
        target: &Cloud115BtFolderSelection,
    ) -> Result<()> {
        self.ensure_web_request_allowed()?;
        let first_path = canonical_manifest_file(output_dir, relative_file)?;
        let first_torrent = load_checked_torrent(output_dir, relative_file, expected_info_hash)?;
        if !first_torrent
            .info_hash
            .eq_ignore_ascii_case(expected_info_hash)
        {
            bail!("本地 Torrent 与期望 Infohash 不一致，拒绝删除");
        }

        let account_id = super::user_id_from_cookie(&self.current_cookie())
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("无法从当前 115 Cookie 会话确认账号标识"))?;
        if account_id != expected_account_id {
            bail!("当前 115 账号与批次账本账号不一致，拒绝删除本地 Torrent");
        }

        let (_, duplicate_status) =
            self.resolve_target_and_check_task(&first_torrent.info_hash, target)?;
        if duplicate_status != "found" {
            bail!("远端未能正向确认相同 Infohash 任务，保留本地 Torrent");
        }

        // Revalidate the canonical path and content after the remote read. If
        // a symlink/path was replaced during the request, it cannot redirect
        // deletion outside the configured EH output directory.
        let final_path = canonical_manifest_file(output_dir, relative_file)?;
        if final_path != first_path {
            bail!("Torrent 路径在远端核对期间发生变化，拒绝删除");
        }
        let final_torrent = load_checked_torrent(output_dir, relative_file, expected_info_hash)?;
        if final_torrent.info_hash != first_torrent.info_hash {
            bail!("Torrent 内容在远端核对期间发生变化，拒绝删除");
        }
        std::fs::remove_file(&final_path).context("删除已确认远端重复项的本地 Torrent 失败")?;
        Ok(())
    }

    /// Uploads exactly one manifest-relative Torrent through the existing
    /// Cookie session, confirms the remote parse against local bencode, and
    /// never repeats a write request after an ambiguous response.
    pub fn bt_upload_and_parse(
        &self,
        output_dir: &str,
        relative_file: &str,
        expected_info_hash: &str,
        target: &Cloud115BtFolderSelection,
    ) -> Result<Cloud115BtUploadParseResult> {
        self.ensure_web_request_allowed()?;
        let path = canonical_manifest_file(output_dir, relative_file)?;
        let torrent_bytes = std::fs::read(&path).context("读取待上传 Torrent 失败")?;
        if torrent_bytes.is_empty() || torrent_bytes.len() > BT_TORRENT_MAX_BYTES as usize {
            bail!("Torrent 文件大小超出允许范围");
        }
        let parsed = parse_torrent_bytes(&torrent_bytes)?;
        if !parsed.info_hash.eq_ignore_ascii_case(expected_info_hash) {
            bail!("待上传 Torrent 的 infohash 与 H0 清单不一致");
        }
        let target_folder_id = self.validate_target_folder_selection(target)?;

        let torrent_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("Torrent 文件名不是有效 UTF-8"))?
            .to_string();
        let torrent_size = torrent_bytes.len() as u64;
        let cookie = self.current_cookie();
        let account_id = super::user_id_from_cookie(&cookie)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("当前 115 Cookie 会话缺少账号标识"))?;

        // A prior upload whose response was lost is first reconciled using the
        // exact target directory, name, and size. A unique candidate is parsed
        // and checked by infohash; it is never uploaded again.
        if let Some(pick_code) =
            self.find_exact_target_file(&target_folder_id, &torrent_name, torrent_size)?
        {
            return self.finish_uploaded_torrent(
                &account_id,
                target,
                &torrent_name,
                torrent_size,
                &parsed,
                &pick_code,
                true,
            );
        }

        let upload_key = bt_operation_key(&account_id, &parsed.info_hash, &target_folder_id);
        if self
            .bt_upload_attempts
            .lock()
            .unwrap()
            .contains(&upload_key)
        {
            return Ok(self.upload_result(
                "UNKNOWN_UPLOAD_RESULT",
                "本进程已有一次上传请求结果未确认；本次只允许继续只读核对，不会再次上传。",
                &account_id,
                target,
                &torrent_name,
                torrent_size,
                &parsed,
                None,
                false,
                Vec::new(),
            ));
        }

        let (status, body) = self.bt_request(
            "GET",
            &self.bt_endpoints.upload_info,
            &[],
            None,
            "",
            true,
            &[],
        )?;
        if let Some((state, message)) = bt_gate_status_result(status) {
            return Ok(self.upload_result(
                state,
                message,
                &account_id,
                target,
                &torrent_name,
                torrent_size,
                &parsed,
                None,
                false,
                Vec::new(),
            ));
        }
        ensure_bt_read_status(self, status, "115 上传身份查询")?;
        let upload_info: serde_json::Value =
            serde_json::from_slice(&body).context("解析 115 上传身份响应失败")?;
        let identity = parse_upload_identity(&upload_info, &account_id)?;

        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .context("读取系统时间失败")?
            .as_secs();
        let mut sign_key = String::new();
        let mut sign_val = String::new();
        let mut second_pass = false;
        let mut upload_attempt_reserved = false;
        let init_response = loop {
            if !upload_attempt_reserved {
                let mut attempts = self.bt_upload_attempts.lock().unwrap();
                if !attempts.insert(upload_key.clone()) {
                    return Ok(self.upload_result(
                        "UNKNOWN_UPLOAD_RESULT",
                        "本进程已有一次上传请求结果未确认；本次只允许继续只读核对，不会再次上传。",
                        &account_id,
                        target,
                        &torrent_name,
                        torrent_size,
                        &parsed,
                        None,
                        false,
                        Vec::new(),
                    ));
                }
                upload_attempt_reserved = true;
            }
            let packet = build_upload_init_packet(&UploadInitInput {
                user_id: &identity.user_id,
                user_key: &identity.user_key,
                file_sha1: &parsed.torrent_sha1,
                filename: &torrent_name,
                file_size: torrent_size,
                target_folder_id: &target_folder_id,
                sign_key: &sign_key,
                sign_val: &sign_val,
                timestamp: now,
            })?;
            let query = [("k_ec", packet.k_ec.clone())];
            let headers = [
                ("content-type", "application/x-www-form-urlencoded"),
                ("accept-encoding", "identity"),
                ("user-agent", UPLOAD_USER_AGENT),
            ];
            let (status, response_body) = match self.bt_request(
                "POST",
                &self.bt_endpoints.upload_init,
                &query,
                Some(&packet.body),
                "application/x-www-form-urlencoded",
                true,
                &headers,
            ) {
                Ok(response) => response,
                Err(_) => {
                    return Ok(self.upload_result(
                        "UNKNOWN_UPLOAD_RESULT",
                        "上传初始化请求已发出但响应未确认；请查询目标目录，不要再次上传。",
                        &account_id,
                        target,
                        &torrent_name,
                        torrent_size,
                        &parsed,
                        None,
                        false,
                        Vec::new(),
                    ));
                }
            };
            if let Some((state, message)) = bt_gate_status_result(status) {
                return Ok(self.upload_result(
                    state,
                    message,
                    &account_id,
                    target,
                    &torrent_name,
                    torrent_size,
                    &parsed,
                    None,
                    false,
                    Vec::new(),
                ));
            }
            if !(200..300).contains(&status) {
                if status >= 500 || (300..400).contains(&status) {
                    return Ok(self.upload_result(
                        "UNKNOWN_UPLOAD_RESULT",
                        format!("上传初始化响应状态为 HTTP {status}；请先查询目标目录，不要重发。"),
                        &account_id,
                        target,
                        &torrent_name,
                        torrent_size,
                        &parsed,
                        None,
                        false,
                        Vec::new(),
                    ));
                }
                bail!("115 上传初始化被拒绝（HTTP {status}）");
            }
            let init_json = match decode_upload_init_response(&response_body) {
                Ok(value) => value,
                Err(_) => {
                    return Ok(self.upload_result(
                        "UNKNOWN_UPLOAD_RESULT",
                        "上传初始化已返回但响应解密或解析失败；请先查询目标目录，不要重发。",
                        &account_id,
                        target,
                        &torrent_name,
                        torrent_size,
                        &parsed,
                        None,
                        false,
                        Vec::new(),
                    ));
                }
            };
            let init = parse_upload_init_result(&init_json)?;
            if init.status == 7 {
                if second_pass {
                    bail!("115 上传二次校验重复返回 status=7；停止继续请求");
                }
                second_pass = true;
                let range = parse_sign_check_range(&init.sign_check, torrent_bytes.len())?;
                sign_key = init.sign_key;
                sign_val = hex_upper(&sha1_bytes(&torrent_bytes[range]));
                // Status 7 is the explicit second-pass handshake defined by
                // the SDK contract. It is not a retry of an unknown request.
                continue;
            }
            break init;
        };

        if init_response.status == 2 {
            return self.finish_uploaded_torrent(
                &account_id,
                target,
                &torrent_name,
                torrent_size,
                &parsed,
                &init_response.pick_code,
                true,
            );
        }
        if init_response.status != 1 {
            bail!("115 上传初始化返回不支持的状态 {}", init_response.status);
        }

        let (token_status, token_body) = self.bt_request(
            "GET",
            &self.bt_endpoints.upload_token,
            &[],
            None,
            "",
            false,
            &[],
        )?;
        if !(200..300).contains(&token_status) {
            bail!("115 OSS 上传凭证请求失败（HTTP {token_status}）");
        }
        let token_json: serde_json::Value =
            serde_json::from_slice(&token_body).context("解析 115 OSS 凭证响应失败")?;
        let token = parse_upload_token(&token_json)?;
        let object_url = build_oss_upload_url_at(
            &self.bt_endpoints.upload_object_base,
            &init_response.bucket,
            &init_response.object,
            cfg!(test),
        )?;
        let parsed_url = reqwest::Url::parse(&object_url).context("OSS 上传地址无效")?;
        let object_path = parsed_url.path();
        let date = httpdate::fmt_http_date(SystemTime::now());
        let callback_header =
            base64::engine::general_purpose::STANDARD.encode(&init_response.callback);
        let callback_var_header =
            base64::engine::general_purpose::STANDARD.encode(&init_response.callback_var);
        let authorization = build_oss_authorization(
            &token,
            "PUT",
            &init_response.bucket,
            object_path,
            &date,
            "application/octet-stream",
            &init_response.callback,
            &init_response.callback_var,
        )?;
        let oss_response = self
            .bt_strict_client
            .put(&object_url)
            .header("authorization", authorization)
            .header("date", date)
            .header("x-oss-security-token", &token.security_token)
            .header("x-oss-callback", callback_header)
            .header("x-oss-callback-var", callback_var_header)
            .header("content-type", "application/octet-stream")
            // Deliberately omit the 115 Cookie. OSS receives only its short-
            // lived upload credentials and callback data.
            .body(torrent_bytes)
            .send();
        let oss_status = match oss_response {
            Ok(response) => response.status().as_u16(),
            Err(_) => {
                return Ok(self.upload_result(
                    "UNKNOWN_UPLOAD_RESULT",
                    "Torrent 上传请求已发出但 OSS 响应未确认；请查询目标目录，不要再次上传。",
                    &account_id,
                    target,
                    &torrent_name,
                    torrent_size,
                    &parsed,
                    Some(init_response.pick_code),
                    false,
                    Vec::new(),
                ));
            }
        };
        if !(200..300).contains(&oss_status) {
            if oss_status >= 500 || (300..400).contains(&oss_status) {
                return Ok(self.upload_result(
                    "UNKNOWN_UPLOAD_RESULT",
                    format!("OSS 上传响应状态为 HTTP {oss_status}；请查询目标目录，不要重发。"),
                    &account_id,
                    target,
                    &torrent_name,
                    torrent_size,
                    &parsed,
                    Some(init_response.pick_code),
                    false,
                    Vec::new(),
                ));
            }
            bail!("OSS 明确拒绝 Torrent 上传（HTTP {oss_status}）");
        }

        match self.finish_uploaded_torrent(
            &account_id,
            target,
            &torrent_name,
            torrent_size,
            &parsed,
            &init_response.pick_code,
            false,
        ) {
            Ok(result) => Ok(result),
            Err(_) => Ok(self.upload_result(
                "UPLOADED_PARSE_PENDING",
                "Torrent 已上传；115 解析结果暂不可用。可以重新查询解析结果，不会重传。",
                &account_id,
                target,
                &torrent_name,
                torrent_size,
                &parsed,
                Some(init_response.pick_code),
                false,
                Vec::new(),
            )),
        }
    }

    /// Query the uploaded Torrent using only its whole-file SHA-1; this is
    /// safe after an upload response is uncertain and never performs a write.
    pub fn bt_confirm_upload_parse(
        &self,
        output_dir: &str,
        relative_file: &str,
        expected_info_hash: &str,
        target: &Cloud115BtFolderSelection,
        pick_code: Option<String>,
    ) -> Result<Cloud115BtUploadParseResult> {
        self.ensure_web_request_allowed()?;
        let parsed = load_checked_torrent(output_dir, relative_file, expected_info_hash)?;
        let cookie = self.current_cookie();
        let account_id = super::user_id_from_cookie(&cookie)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("当前 115 Cookie 会话缺少账号标识"))?;
        let target_folder_id = self.validate_target_folder_selection(target)?;
        let torrent_name = Path::new(relative_file)
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow!("Torrent 文件名不是有效 UTF-8"))?
            .to_string();
        let size = std::fs::metadata(canonical_manifest_file(output_dir, relative_file)?)?.len();
        let pick_code = match pick_code {
            Some(value) if !value.trim().is_empty() => value,
            _ => self
                .find_exact_target_file(&target_folder_id, &torrent_name, size)?
                .ok_or_else(|| anyhow!("目标目录尚未发现该 Torrent；上传结果仍未知，不要重发"))?,
        };
        self.finish_uploaded_torrent(
            &account_id,
            target,
            &torrent_name,
            size,
            &parsed,
            &pick_code,
            true,
        )
    }

    /// One explicit BT POST. On a lost/ambiguous response it checks the task
    /// list and returns unknown if absence cannot yet be proven; it never
    /// retries the POST.
    pub fn bt_submit_once(
        &self,
        output_dir: &str,
        relative_file: &str,
        expected_info_hash: &str,
        target: &Cloud115BtFolderSelection,
        wanted_indices: &[u32],
    ) -> Result<Cloud115BtSubmitResult> {
        self.ensure_web_request_allowed()?;
        let parsed = load_checked_torrent(output_dir, relative_file, expected_info_hash)?;
        let wanted = validate_wanted_indices(wanted_indices, parsed.files.len())?;
        let target_folder_id = self.validate_target_folder_selection(target)?;

        let cookie = self.current_cookie();
        let account_id = super::user_id_from_cookie(&cookie)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("当前 115 Cookie 会话缺少账号标识"))?;
        let submit_key = bt_operation_key(&account_id, &parsed.info_hash, &target_folder_id);
        if self
            .bt_submit_attempts
            .lock()
            .unwrap()
            .contains(&submit_key)
        {
            return Ok(Cloud115BtSubmitResult {
                state: "UNKNOWN_REMOTE_STATE".into(),
                info_hash: parsed.info_hash,
                message: "本进程此前的 BT 提交结果仍未确认；为避免重复写入，本次不会再次提交。"
                    .into(),
                target_folder_id,
            });
        }
        if !self.bt_submit_attempts.lock().unwrap().insert(submit_key) {
            return Ok(Cloud115BtSubmitResult {
                state: "UNKNOWN_REMOTE_STATE".into(),
                info_hash: parsed.info_hash,
                message: "本进程中的另一项 BT 提交正在确认；本次不会再次提交。".into(),
                target_folder_id,
            });
        }

        let payload = build_ssp_bt_payload_json(&parsed.info_hash, &wanted, "", &target_folder_id);
        let body = build_ssp_bt_form_body(&payload);
        let headers = [
            ("content-type", "application/x-www-form-urlencoded"),
            ("user-agent", BT_SSP_USER_AGENT),
            ("referer", "https://clouddownload.115.com/"),
        ];
        let response = self.bt_request(
            "POST",
            &self.bt_endpoints.submit_bt,
            &[],
            Some(body.as_bytes()),
            "application/x-www-form-urlencoded",
            true,
            &headers,
        );
        let (state, message) = match response {
            Err(_) => self.reconcile_bt_submit(&parsed.info_hash, &target_folder_id),
            Ok((405, _)) => (
                "WAF_BLOCKED".to_string(),
                "115 返回 WAF 405；已停止后台请求并进入冷却。".to_string(),
            ),
            Ok((429, _)) => (
                "RATE_LIMITED".into(),
                "115 返回 HTTP 429；没有重试。".into(),
            ),
            Ok((401, _)) => (
                "AUTH_EXPIRED".into(),
                "115 Cookie 会话鉴权失败（HTTP 401）；已停止本批次写入。".into(),
            ),
            Ok((403, _)) => (
                "ACCESS_DENIED".into(),
                "115 拒绝当前 Cookie 会话访问（HTTP 403）；已停止本批次写入。".into(),
            ),
            Ok((status, response_body)) if (200..300).contains(&status) => {
                let response = parse_ssp_response(&response_body).ok();
                match response
                    .as_ref()
                    .and_then(|value| value.get("state"))
                    .and_then(serde_json::Value::as_bool)
                {
                    Some(true) => (
                        "ACCEPTED".into(),
                        "115 已接受 BT 提交请求；云端下载状态尚未确认。".into(),
                    ),
                    Some(false) => (
                        "REJECTED".into(),
                        response
                            .as_ref()
                            .map(bt_rejection_message)
                            .unwrap_or_default(),
                    ),
                    None => self.reconcile_bt_submit(&parsed.info_hash, &target_folder_id),
                }
            }
            Ok((status, _)) if status >= 500 || (300..400).contains(&status) => {
                self.reconcile_bt_submit(&parsed.info_hash, &target_folder_id)
            }
            Ok((status, response_body)) => {
                let explicit = parse_ssp_response(&response_body).ok();
                if explicit
                    .as_ref()
                    .and_then(|value| value.get("state"))
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                {
                    self.reconcile_bt_submit(&parsed.info_hash, &target_folder_id)
                } else {
                    (
                        "REJECTED".into(),
                        format!("115 明确拒绝 BT 提交（HTTP {status}）。"),
                    )
                }
            }
        };
        Ok(Cloud115BtSubmitResult {
            state,
            info_hash: parsed.info_hash,
            message,
            target_folder_id,
        })
    }

    pub fn bt_task_status(
        &self,
        info_hash: &str,
        target: &Cloud115BtFolderSelection,
    ) -> Result<Cloud115BtTaskStatus> {
        self.ensure_web_request_allowed()?;
        let target_folder_id = self.validate_target_folder_selection(target)?;
        self.read_task_status(info_hash, &target_folder_id)
    }

    /// Reads one stable, bounded task-list snapshot through the current Cookie
    /// session and request/WAF gate. No write endpoint is reachable here.
    pub fn bt_task_snapshot(&self) -> Result<Cloud115BtTaskSnapshot> {
        self.ensure_web_request_allowed()?;
        let account_id = super::user_id_from_cookie(&self.current_cookie())
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| anyhow!("无法从当前 115 Cookie 会话确认账号标识"))?;
        let mut expected_count = None;
        let mut expected_page_count = None;
        let mut tasks = BTreeMap::<(String, Option<String>), Cloud115BtTaskSnapshotItem>::new();
        let mut pages_read = 0;

        for page_number in 1..=BT_TASK_PAGE_BUDGET {
            let query = vec![
                ("ac", "task_lists".to_string()),
                ("page", page_number.to_string()),
                ("page_size", BT_TASK_PAGE_SIZE.to_string()),
            ];
            let (status, body) = self.bt_request(
                "GET",
                &self.bt_endpoints.task_list,
                &query,
                None,
                "",
                true,
                &[],
            )?;
            pages_read += 1;
            if let Some((state, message)) = bt_gate_status_result(status) {
                return Ok(Cloud115BtTaskSnapshot {
                    account_id,
                    state: state.to_string(),
                    complete: false,
                    pages_read,
                    error_class: Some(state.to_ascii_lowercase()),
                    message: message.to_string(),
                    tasks: tasks.into_values().collect(),
                });
            }
            ensure_bt_read_status(self, status, "115 离线任务列表")?;
            let response: serde_json::Value =
                serde_json::from_slice(&body).context("解析 115 离线任务列表失败")?;
            let page = match parse_task_list_page(&response, "") {
                Ok(page) => page,
                Err(error) => {
                    return Ok(Cloud115BtTaskSnapshot {
                        account_id,
                        state: "unknown".into(),
                        complete: false,
                        pages_read,
                        error_class: Some("malformed_response".into()),
                        message: error.to_string().chars().take(180).collect(),
                        tasks: tasks.into_values().collect(),
                    });
                }
            };
            if let (Some(count), Some(page_count)) = (expected_count, expected_page_count) {
                if count != page.count || page_count != page.page_count {
                    return Ok(Cloud115BtTaskSnapshot {
                        account_id,
                        state: "partial".into(),
                        complete: false,
                        pages_read,
                        error_class: Some("pagination_unstable".into()),
                        message: "任务列表翻页期间总数或总页数发生变化".into(),
                        tasks: tasks.into_values().collect(),
                    });
                }
            } else {
                expected_count = Some(page.count);
                expected_page_count = Some(page.page_count);
            }
            for task in page.tasks {
                tasks
                    .entry((task.info_hash.clone(), task.target_folder_id.clone()))
                    .or_insert(task);
            }
            if page.page_count > BT_TASK_PAGE_BUDGET {
                return Ok(Cloud115BtTaskSnapshot {
                    account_id,
                    state: "partial".into(),
                    complete: false,
                    pages_read,
                    error_class: Some("query_budget_exhausted".into()),
                    message: "任务列表超过 300 项安全查询上限，未读取页面不作缺失判断".into(),
                    tasks: tasks.into_values().collect(),
                });
            }
            if page.page_count == 0 || page_number >= page.page_count {
                return Ok(Cloud115BtTaskSnapshot {
                    account_id,
                    state: "ok".into(),
                    complete: true,
                    pages_read,
                    error_class: None,
                    message: String::new(),
                    tasks: tasks.into_values().collect(),
                });
            }
        }
        Ok(Cloud115BtTaskSnapshot {
            account_id,
            state: "partial".into(),
            complete: false,
            pages_read,
            error_class: Some("query_budget_exhausted".into()),
            message: "安全查询预算内未读完任务列表".into(),
            tasks: tasks.into_values().collect(),
        })
    }

    fn bt_request(
        &self,
        method: &str,
        url: &str,
        query: &[(&str, String)],
        body: Option<&[u8]>,
        content_type: &str,
        with_cookie: bool,
        headers: &[(&str, &str)],
    ) -> Result<(u16, Vec<u8>)> {
        self.ensure_web_request_allowed()?;
        let gate_guard = self.gate.enter();
        crate::source::record_gate_wait(self.gate.channel(), gate_guard.waited_us());
        self.ensure_web_request_allowed()?;
        let method =
            reqwest::Method::from_bytes(method.as_bytes()).context("115 BT 请求方法无效")?;
        let mut request = self
            .bt_strict_client
            .request(method, url)
            .query(query)
            .header(reqwest::header::USER_AGENT, BT_SSP_USER_AGENT)
            .header(reqwest::header::ACCEPT_ENCODING, "identity");
        if with_cookie {
            request = request.header(reqwest::header::COOKIE, self.current_cookie());
        }
        if !content_type.is_empty() {
            request = request.header(reqwest::header::CONTENT_TYPE, content_type);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        if let Some(body) = body {
            request = request.body(body.to_vec());
        }
        let response = request.send().context("115 BT 网络请求结果未确认")?;
        let status = response.status().as_u16();
        if status == 405 {
            self.mark_web_waf_blocked();
        }
        let body = response.bytes().context("读取 115 BT 响应失败")?.to_vec();
        if body.len() > UPLOAD_RESPONSE_MAX_BYTES {
            bail!("115 BT 响应超过大小限制");
        }
        Ok((status, body))
    }

    fn read_task_status(
        &self,
        info_hash: &str,
        target_folder_id: &str,
    ) -> Result<Cloud115BtTaskStatus> {
        let mut expected_count = None;
        let mut expected_page_count = None;
        let mut saw_unverified_match = false;
        for page_number in 1..=BT_TASK_PAGE_BUDGET {
            let query = vec![
                ("ac", "task_lists".to_string()),
                ("page", page_number.to_string()),
                ("page_size", BT_TASK_PAGE_SIZE.to_string()),
            ];
            let (status, body) = self.bt_request(
                "GET",
                &self.bt_endpoints.task_list,
                &query,
                None,
                "",
                true,
                &[],
            )?;
            if let Some((state, message)) = bt_gate_status_result(status) {
                return Ok(Cloud115BtTaskStatus {
                    state: state.to_string(),
                    info_hash: info_hash.to_ascii_lowercase(),
                    status_code: Some(i64::from(status)),
                    status_text: message.to_string(),
                    percent_done: None,
                    target_folder_id: String::new(),
                });
            }
            ensure_bt_read_status(self, status, "115 离线任务列表")?;
            let response: serde_json::Value =
                serde_json::from_slice(&body).context("解析 115 离线任务列表失败")?;
            let page = parse_task_list_page(&response, info_hash)?;
            if let (Some(count), Some(page_count)) = (expected_count, expected_page_count) {
                if count != page.count || page_count != page.page_count {
                    return Ok(unknown_task_status(
                        info_hash,
                        "任务列表翻页期间总数或总页数发生变化，无法证明不存在同 Infohash 任务。",
                    ));
                }
            } else {
                expected_count = Some(page.count);
                expected_page_count = Some(page.page_count);
                if page.count == 0 && page.page_count <= 1 {
                    return Ok(not_found_task_status(info_hash));
                }
            }
            for matched in page.matched_tasks {
                if matched.target_folder_id.as_deref() == Some(target_folder_id) {
                    return Ok(Cloud115BtTaskStatus {
                        state: "found".into(),
                        info_hash: info_hash.to_ascii_lowercase(),
                        status_code: matched.status_code,
                        status_text: matched.status_text,
                        percent_done: matched.percent_done,
                        target_folder_id: matched.target_folder_id.unwrap_or_default(),
                    });
                }
                saw_unverified_match = true;
            }
            if page.page_count > BT_TASK_PAGE_BUDGET {
                return Ok(unknown_task_status(
                    info_hash,
                    "任务列表超过安全查询上限（最多 300 项），未读取的页面仍可能有同哈希任务。",
                ));
            }
            if page_number >= page.page_count {
                return Ok(if saw_unverified_match {
                    unknown_task_status(
                        info_hash,
                        "发现同 Infohash 任务，但其目标目录缺失或与所选目录不一致。",
                    )
                } else {
                    not_found_task_status(info_hash)
                });
            }
        }
        Ok(unknown_task_status(
            info_hash,
            "在安全查询预算内未能读完整个任务列表，无法证明不存在同 Infohash 任务。",
        ))
    }

    fn reconcile_bt_submit(&self, info_hash: &str, target_folder_id: &str) -> (String, String) {
        match self.read_task_status(info_hash, target_folder_id) {
            Ok(status) if status.state == "found" => (
                "REMOTE_TASK_FOUND".into(),
                "只读核实到远端同 Infohash 任务；未发送第二次提交。".into(),
            ),
            Ok(status) if status.state == "not_found" => (
                "UNKNOWN_REMOTE_STATE".into(),
                "远端暂未找到任务；提交结果仍未确认，请稍后只读查询，不要重新提交。".into(),
            ),
            Ok(status)
                if matches!(
                    status.state.as_str(),
                    "WAF_BLOCKED" | "RATE_LIMITED" | "AUTH_EXPIRED" | "ACCESS_DENIED"
                ) =>
            {
                (status.state, status.status_text)
            }
            Ok(status) => (
                "UNKNOWN_REMOTE_STATE".into(),
                if status.status_text.trim().is_empty() {
                    "提交结果尚未确认；任务列表无法给出确定结论，禁止再次提交。".into()
                } else {
                    format!(
                        "提交结果尚未确认：{}；禁止再次提交。",
                        status.status_text.trim()
                    )
                },
            ),
            Err(_) => (
                "UNKNOWN_REMOTE_STATE".into(),
                "提交结果尚未确认；只读任务查询失败，禁止再次提交。".into(),
            ),
        }
    }

    fn finish_uploaded_torrent(
        &self,
        account_id: &str,
        target: &Cloud115BtFolderSelection,
        torrent_name: &str,
        torrent_size: u64,
        parsed: &ParsedTorrent,
        pick_code: &str,
        upload_reused: bool,
    ) -> Result<Cloud115BtUploadParseResult> {
        let files = match self.parse_uploaded_torrent(parsed)? {
            BtUploadParseOutcome::Parsed(files) => files,
            BtUploadParseOutcome::Blocked(state, message) => {
                return Ok(self.upload_result(
                    state,
                    message,
                    account_id,
                    target,
                    torrent_name,
                    torrent_size,
                    parsed,
                    Some(pick_code.to_string()),
                    upload_reused,
                    Vec::new(),
                ));
            }
        };
        Ok(self.upload_result(
            "PARSED",
            if upload_reused {
                "远端已有相同 SHA-1 Torrent 文件；已复用并核实解析结果。"
            } else {
                "Torrent 上传完成，远端解析与本地 infohash/文件清单一致。"
            },
            account_id,
            target,
            torrent_name,
            torrent_size,
            parsed,
            Some(pick_code.to_string()),
            upload_reused,
            files,
        ))
    }

    fn parse_uploaded_torrent(&self, parsed: &ParsedTorrent) -> Result<BtUploadParseOutcome> {
        let query = vec![
            ("ac", "torrent".to_string()),
            ("sha1", parsed.torrent_sha1.clone()),
        ];
        let (status, body) = self.bt_request(
            "GET",
            &self.bt_endpoints.torrent_info,
            &query,
            None,
            "",
            true,
            &[],
        )?;
        if let Some((state, message)) = bt_gate_status_result(status) {
            return Ok(BtUploadParseOutcome::Blocked(state, message));
        }
        ensure_bt_read_status(self, status, "115 Torrent 解析")?;
        let response: serde_json::Value =
            serde_json::from_slice(&body).context("解析 115 Torrent 响应失败")?;
        parse_remote_torrent_parse(&response, &parsed.info_hash, &parsed.files)
            .map(BtUploadParseOutcome::Parsed)
    }

    fn upload_result(
        &self,
        state: &str,
        message: impl Into<String>,
        account_id: &str,
        target: &Cloud115BtFolderSelection,
        torrent_name: &str,
        torrent_size: u64,
        parsed: &ParsedTorrent,
        pick_code: Option<String>,
        upload_reused: bool,
        files: Vec<Cloud115BtFile>,
    ) -> Cloud115BtUploadParseResult {
        Cloud115BtUploadParseResult {
            state: state.to_string(),
            message: message.into(),
            account_id: account_id.to_string(),
            target_folder_path: target.path.clone(),
            target_folder_id: target.id.clone(),
            torrent_name: torrent_name.to_string(),
            torrent_size,
            torrent_sha1: parsed.torrent_sha1.clone(),
            info_hash: parsed.info_hash.clone(),
            pick_code,
            upload_reused,
            files,
        }
    }

    fn find_exact_target_file(
        &self,
        folder_id: &str,
        filename: &str,
        file_size: u64,
    ) -> Result<Option<String>> {
        let mut offset = 0i64;
        let mut matches = Vec::new();
        for _ in 0..BT_FOLDER_PAGE_BUDGET {
            let query = vec![
                ("aid", "1".to_string()),
                ("cid", folder_id.to_string()),
                ("o", "user_ptime".to_string()),
                ("asc", "0".to_string()),
                ("offset", offset.to_string()),
                ("show_dir", "1".to_string()),
                ("limit", WEB_PAGE_SIZE.to_string()),
                ("snap", "0".to_string()),
                ("natsort", "0".to_string()),
                ("record_open_time", "0".to_string()),
                ("format", "json".to_string()),
                ("fc_mix", "0".to_string()),
            ];
            let (status, body) = self.bt_request(
                "GET",
                &self.bt_endpoints.files_list,
                &query,
                None,
                "",
                true,
                &[],
            )?;
            ensure_bt_read_status(self, status, "115 目标目录文件列表")?;
            let page: WebFilesResp =
                serde_json::from_slice(&body).context("解析 115 目标目录列表失败")?;
            if !page.state || page.code != 0 {
                bail!(
                    "115 目标目录列表返回业务错误（errno={}）",
                    page.errno.max(page.code)
                );
            }
            let page_len = page.data.len() as i64;
            for item in page.data {
                let is_file = !item.fid.is_empty();
                if is_file && item.name == filename && item.s.as_u64() == file_size {
                    if item.pc.is_empty() {
                        bail!("115 目标目录中存在同名同大小文件，但缺少 pickcode；停止重复上传");
                    }
                    matches.push(item.pc);
                }
            }
            if page.count <= offset + page_len {
                break;
            }
            offset = offset.saturating_add(WEB_PAGE_SIZE);
            if offset >= BT_FOLDER_PAGE_BUDGET * WEB_PAGE_SIZE {
                bail!("115 目标目录列表超过安全查询预算；停止上传");
            }
        }
        matches.sort();
        matches.dedup();
        if matches.len() > 1 {
            bail!("115 目标目录存在多个同名同大小 Torrent；停止重复上传");
        }
        Ok(matches.into_iter().next())
    }

    fn resolve_target_and_check_task(
        &self,
        info_hash: &str,
        target: &Cloud115BtFolderSelection,
    ) -> Result<(String, String)> {
        let target_folder_id = self.validate_target_folder_selection(target)?;
        let duplicate_status = self.read_task_status(info_hash, &target_folder_id)?.state;
        Ok((target_folder_id, duplicate_status))
    }

    fn validate_target_folder_selection(
        &self,
        target: &Cloud115BtFolderSelection,
    ) -> Result<String> {
        if target.id == "0"
            && target.parent_id.is_empty()
            && target.name.is_empty()
            && target.path == "/"
        {
            return Ok("0".to_string());
        }
        if target.id.is_empty()
            || !target.id.bytes().all(|byte| byte.is_ascii_digit())
            || target.id == "0"
            || target.parent_id.is_empty()
            || !target.parent_id.bytes().all(|byte| byte.is_ascii_digit())
            || target.name.trim().is_empty()
            || !target.path.starts_with('/')
            || target.path.len() < 2
            || target.path.ends_with('/')
            || target.path.rsplit('/').next() != Some(target.name.as_str())
            || target
                .path
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            bail!("所选 115 目录信息无效，请重新浏览并选择目录");
        }

        let candidates = self.read_folder_children(&target.parent_id)?;
        let selected = select_exact_folder(&candidates, &target.name)?;
        if selected.id != target.id {
            bail!("所选 115 目录已变化，请重新浏览并选择目录");
        }
        Ok(selected.id)
    }

    fn read_folder_children(&self, parent_id: &str) -> Result<Vec<FolderCandidate>> {
        if parent_id.is_empty() || !parent_id.bytes().all(|byte| byte.is_ascii_digit()) {
            bail!("115 目录浏览的父目录 ID 无效");
        }
        let mut folders = Vec::new();
        let mut offset = 0i64;
        for _ in 0..BT_FOLDER_PAGE_BUDGET {
            let query = vec![
                ("aid", "1".to_string()),
                ("cid", parent_id.to_string()),
                ("o", "user_ptime".to_string()),
                ("asc", "0".to_string()),
                ("offset", offset.to_string()),
                ("show_dir", "1".to_string()),
                ("limit", WEB_PAGE_SIZE.to_string()),
                ("snap", "0".to_string()),
                ("natsort", "0".to_string()),
                // Read-only listing: do not mutate the directory's open time.
                ("record_open_time", "0".to_string()),
                ("format", "json".to_string()),
                ("fc_mix", "0".to_string()),
            ];
            let (status, body) = self.bt_request(
                "GET",
                &self.bt_endpoints.files_list,
                &query,
                None,
                "",
                true,
                &[],
            )?;
            ensure_bt_read_status(self, status, "115 文件夹列表")?;
            let page: WebFilesResp =
                serde_json::from_slice(&body).context("解析 115 文件夹列表响应失败")?;
            if !page.state || page.code != 0 {
                bail!(
                    "115 文件夹列表返回业务错误（errno={}）",
                    page.errno.max(page.code)
                );
            }
            if page.count < 0 {
                bail!("115 文件夹列表返回无效的目录数量");
            }
            let page_len = page.data.len() as i64;
            folders.extend(page.data.into_iter().filter_map(|item| {
                let is_dir = item.fid.is_empty();
                (is_dir
                    && !item.name.trim().is_empty()
                    && !item.cid.is_empty()
                    && item.cid.bytes().all(|byte| byte.is_ascii_digit()))
                .then_some(FolderCandidate {
                    name: item.name,
                    id: item.cid,
                    is_dir,
                })
            }));
            if page.count <= offset + page_len {
                break;
            }
            offset = offset.saturating_add(WEB_PAGE_SIZE);
            if offset >= BT_FOLDER_PAGE_BUDGET * WEB_PAGE_SIZE {
                bail!("115 目录项超过有界查询预算，无法安全确认目标目录唯一性");
            }
        }
        folders.sort_by(|left, right| left.name.cmp(&right.name).then(left.id.cmp(&right.id)));
        Ok(folders)
    }
}

struct ParsedTaskPage {
    count: u64,
    page_count: u64,
    matched_tasks: Vec<MatchedTask>,
    tasks: Vec<Cloud115BtTaskSnapshotItem>,
}

struct MatchedTask {
    target_folder_id: Option<String>,
    status_code: Option<i64>,
    status_text: String,
    percent_done: Option<f64>,
}

fn parse_task_list_page(
    response: &serde_json::Value,
    expected_info_hash: &str,
) -> Result<ParsedTaskPage> {
    if response.get("state").and_then(serde_json::Value::as_bool) != Some(true) {
        bail!("115 离线任务列表未返回成功状态");
    }
    let count = response
        .get("count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("115 离线任务列表缺少 count"))?;
    let page_count = response
        .get("page_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow!("115 离线任务列表缺少 page_count"))?;
    let tasks = response
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow!("115 离线任务列表缺少 tasks"))?;
    if (count == 0 && (!tasks.is_empty() || page_count > 1)) || (count > 0 && page_count == 0) {
        bail!("115 离线任务列表分页信息不一致");
    }
    let mut matched_tasks = Vec::new();
    let mut parsed_tasks = Vec::with_capacity(tasks.len());
    for task in tasks {
        let Some(task_hash) = task.get("info_hash").and_then(serde_json::Value::as_str) else {
            bail!("115 离线任务项缺少 info_hash，不能安全完成去重");
        };
        let status_code = task
            .get("status")
            .or_else(|| task.get("status_code"))
            .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()));
        let percent_done = task
            .get("percent_done")
            .or_else(|| task.get("progress"))
            .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()));
        // wp_path_id identifies the task's parent; file_id identifies its
        // output and cannot prove its parent folder.
        let target_folder_id = task
            .get("wp_path_id")
            .and_then(|value| {
                value
                    .as_str()
                    .map(str::to_owned)
                    .or_else(|| value.as_u64().map(|id| id.to_string()))
            })
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()));
        parsed_tasks.push(Cloud115BtTaskSnapshotItem {
            info_hash: task_hash.to_ascii_lowercase(),
            target_folder_id: target_folder_id.clone(),
            status_code,
            percent_done,
        });
        if !expected_info_hash.is_empty() && task_hash.eq_ignore_ascii_case(expected_info_hash) {
            let status_text = task
                .get("status_text")
                .or_else(|| task.get("status_name"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| {
                    task.get("status")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| value.parse::<i64>().is_err())
                })
                .unwrap_or_default()
                .chars()
                .take(256)
                .collect();
            matched_tasks.push(MatchedTask {
                target_folder_id,
                status_code,
                status_text,
                percent_done,
            });
        }
    }
    Ok(ParsedTaskPage {
        count,
        page_count,
        matched_tasks,
        tasks: parsed_tasks,
    })
}

fn canonical_manifest_file(output_dir: &str, relative_file: &str) -> Result<PathBuf> {
    let base_input = Path::new(output_dir);
    let base = base_input
        .canonicalize()
        .context("EH Torrent 保存目录不可访问")?;
    let relative = Path::new(relative_file);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("manifest Torrent 相对路径无效");
    }
    let joined = base.join(relative);
    let metadata =
        std::fs::symlink_metadata(&joined).context("manifest Torrent 文件不存在或不可访问")?;
    if !metadata.file_type().is_file() {
        bail!("manifest Torrent 不是普通文件或是符号链接");
    }
    let canonical = joined.canonicalize().context("解析 Torrent 文件路径失败")?;
    if !canonical.starts_with(&base) {
        bail!("Torrent 文件路径越出 EH 保存目录");
    }
    if !canonical
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("torrent"))
    {
        bail!("所选 manifest 文件不是 .torrent");
    }
    Ok(canonical)
}

fn load_checked_torrent(
    output_dir: &str,
    relative_file: &str,
    expected_info_hash: &str,
) -> Result<ParsedTorrent> {
    let path = canonical_manifest_file(output_dir, relative_file)?;
    let metadata = std::fs::metadata(&path)?;
    if metadata.len() == 0 || metadata.len() > BT_TORRENT_MAX_BYTES {
        bail!("Torrent 文件大小超出允许范围");
    }
    let bytes = std::fs::read(path).context("读取所选 Torrent 失败")?;
    let parsed = parse_torrent_bytes(&bytes)?;
    if !parsed.info_hash.eq_ignore_ascii_case(expected_info_hash) {
        bail!("所选 Torrent 的 Infohash 与 H0 清单不匹配");
    }
    Ok(parsed)
}

#[derive(Debug)]
struct ParsedBencode {
    info: BValue,
    info_range: std::ops::Range<usize>,
}

#[derive(Debug)]
enum BValue {
    Bytes(Vec<u8>),
    Integer(i64),
    List(Vec<BValue>),
    Dict(BTreeMap<Vec<u8>, BValue>),
}

struct BParser<'a> {
    bytes: &'a [u8],
    position: usize,
    values: usize,
}

impl<'a> BParser<'a> {
    fn parse_top(mut self) -> Result<ParsedBencode> {
        if self.bytes.first() != Some(&b'd') {
            bail!("Torrent 顶层必须是 bencode 字典");
        }
        self.position += 1;
        let mut previous: Option<Vec<u8>> = None;
        let mut info = None;
        let mut info_range = None;
        while self.peek()? != b'e' {
            let key = self.read_bytes()?.to_vec();
            if previous.as_ref().is_some_and(|previous| previous >= &key) {
                bail!("Torrent 字典键不是规范排序或存在重复键");
            }
            previous = Some(key.clone());
            let start = self.position;
            let value = self.parse_value(1)?;
            let end = self.position;
            if key == b"info" {
                if !matches!(value, BValue::Dict(_)) || info.is_some() {
                    bail!("Torrent info 字段无效");
                }
                info = Some(value);
                info_range = Some(start..end);
            }
        }
        self.position += 1;
        if self.position != self.bytes.len() {
            bail!("Torrent 文件尾部存在多余数据");
        }
        Ok(ParsedBencode {
            info: info.ok_or_else(|| anyhow!("Torrent 缺少 info 字典"))?,
            info_range: info_range.ok_or_else(|| anyhow!("Torrent 缺少 info 字典"))?,
        })
    }

    fn parse_value(&mut self, depth: usize) -> Result<BValue> {
        self.values += 1;
        if depth > 64 || self.values > 200_000 {
            bail!("Torrent bencode 结构超出安全限制");
        }
        match self.peek()? {
            b'i' => {
                self.position += 1;
                let start = self.position;
                while self.peek()? != b'e' {
                    self.position += 1;
                }
                let digits = std::str::from_utf8(&self.bytes[start..self.position])?;
                if digits.is_empty()
                    || digits == "-0"
                    || digits.starts_with("-0")
                    || digits.starts_with('0') && digits.len() > 1
                {
                    bail!("Torrent bencode 整数格式无效");
                }
                let value = digits.parse::<i64>().context("Torrent 整数超出范围")?;
                self.position += 1;
                Ok(BValue::Integer(value))
            }
            b'l' => {
                self.position += 1;
                let mut values = Vec::new();
                while self.peek()? != b'e' {
                    values.push(self.parse_value(depth + 1)?);
                }
                self.position += 1;
                Ok(BValue::List(values))
            }
            b'd' => {
                self.position += 1;
                let mut values = BTreeMap::new();
                let mut previous: Option<Vec<u8>> = None;
                while self.peek()? != b'e' {
                    let key = self.read_bytes()?.to_vec();
                    if previous.as_ref().is_some_and(|previous| previous >= &key) {
                        bail!("Torrent 字典键不是规范排序或存在重复键");
                    }
                    previous = Some(key.clone());
                    values.insert(key, self.parse_value(depth + 1)?);
                }
                self.position += 1;
                Ok(BValue::Dict(values))
            }
            b'0'..=b'9' => Ok(BValue::Bytes(self.read_bytes()?.to_vec())),
            _ => bail!("Torrent bencode 类型标记无效"),
        }
    }

    fn read_bytes(&mut self) -> Result<&'a [u8]> {
        let start = self.position;
        while self
            .bytes
            .get(self.position)
            .is_some_and(u8::is_ascii_digit)
        {
            self.position += 1;
        }
        if self.position == start || self.peek()? != b':' {
            bail!("Torrent bencode 字符串长度无效");
        }
        let digits = std::str::from_utf8(&self.bytes[start..self.position])?;
        if digits.len() > 1 && digits.starts_with('0') {
            bail!("Torrent bencode 字符串长度不是规范格式");
        }
        let length = digits
            .parse::<usize>()
            .context("Torrent 字符串长度超出范围")?;
        self.position += 1;
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| anyhow!("Torrent 字符串长度溢出"))?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| anyhow!("Torrent bencode 字符串截断"))?;
        self.position = end;
        Ok(value)
    }

    fn peek(&self) -> Result<u8> {
        self.bytes
            .get(self.position)
            .copied()
            .ok_or_else(|| anyhow!("Torrent bencode 数据截断"))
    }
}

fn parse_torrent_bytes(bytes: &[u8]) -> Result<ParsedTorrent> {
    if bytes.is_empty() || bytes.len() as u64 > BT_TORRENT_MAX_BYTES {
        bail!("Torrent 文件大小超出允许范围");
    }
    let parsed = BParser {
        bytes,
        position: 0,
        values: 0,
    }
    .parse_top()?;
    let info = match parsed.info {
        BValue::Dict(info) => info,
        _ => bail!("Torrent info 字段不是字典"),
    };
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        &bytes[parsed.info_range],
    );
    let info_hash = to_hex(digest.as_ref());
    let files = parse_torrent_file_entries(&info)?;
    let whole_digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes);
    Ok(ParsedTorrent {
        info_hash,
        torrent_sha1: to_hex(whole_digest.as_ref()),
        files,
    })
}

fn parse_torrent_file_entries(info: &BTreeMap<Vec<u8>, BValue>) -> Result<Vec<Cloud115BtFile>> {
    let mut files = Vec::new();
    if let Some(BValue::List(entries)) = info.get(b"files".as_slice()) {
        if entries.is_empty() || entries.len() > 5000 {
            bail!("Torrent 文件清单数量超出安全范围");
        }
        for (index, entry) in entries.iter().enumerate() {
            let BValue::Dict(entry) = entry else {
                bail!("Torrent 多文件项格式无效")
            };
            let size = match entry.get(b"length".as_slice()) {
                Some(BValue::Integer(value)) if *value >= 0 => *value as u64,
                _ => bail!("Torrent 文件长度无效"),
            };
            let path = match entry.get(b"path".as_slice()) {
                Some(BValue::List(parts)) if !parts.is_empty() => parts
                    .iter()
                    .map(|part| match part {
                        BValue::Bytes(bytes) => safe_torrent_path_component(bytes),
                        _ => bail!("Torrent 文件路径项无效"),
                    })
                    .collect::<Result<Vec<_>>>()?
                    .join("/"),
                _ => bail!("Torrent 文件路径缺失"),
            };
            files.push(Cloud115BtFile {
                index: index as u32,
                path,
                size,
            });
        }
    } else {
        let size = match info.get(b"length".as_slice()) {
            Some(BValue::Integer(value)) if *value >= 0 => *value as u64,
            _ => bail!("Torrent 单文件长度无效"),
        };
        let name = match info.get(b"name".as_slice()) {
            Some(BValue::Bytes(bytes)) => safe_torrent_path_component(bytes)?,
            _ => bail!("Torrent 单文件名称缺失"),
        };
        files.push(Cloud115BtFile {
            index: 0,
            path: name,
            size,
        });
    }
    Ok(files)
}

fn safe_torrent_path_component(bytes: &[u8]) -> Result<String> {
    let value = std::str::from_utf8(bytes).context("Torrent 文件路径不是有效 UTF-8")?;
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.contains('\0')
    {
        bail!("Torrent 文件路径包含不安全的路径段");
    }
    Ok(value.to_string())
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_upper(bytes: &[u8]) -> String {
    to_hex(bytes).to_ascii_uppercase()
}

fn sha1_bytes(bytes: &[u8]) -> [u8; 20] {
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes);
    let mut output = [0u8; 20];
    output.copy_from_slice(digest.as_ref());
    output
}

fn crc32_ieee(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn encode_upload_ecdh_token(timestamp: u64) -> Result<String> {
    let timestamp = u32::try_from(timestamp).context("上传时间戳超出协议范围")?;
    let mut token = Vec::with_capacity(48);
    token.extend_from_slice(&UPLOAD_ECDH_PUBKEY[..15]);
    token.extend_from_slice(&[0, b's', 0, 0, 0]);
    token.extend_from_slice(&timestamp.to_le_bytes());
    token.extend_from_slice(&UPLOAD_ECDH_PUBKEY[15..]);
    token.extend_from_slice(&[0, 1, 0, 0, 0]);
    let mut crc_data = Vec::with_capacity(UPLOAD_CRC_SALT.len() + token.len());
    crc_data.extend_from_slice(UPLOAD_CRC_SALT);
    crc_data.extend_from_slice(&token);
    token.extend_from_slice(&crc32_ieee(&crc_data).to_le_bytes());
    Ok(base64::engine::general_purpose::STANDARD.encode(token))
}

fn form_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                encoded.push(char::from(byte));
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push('%');
                encoded.push(char::from(b"0123456789ABCDEF"[(byte >> 4) as usize]));
                encoded.push(char::from(b"0123456789ABCDEF"[(byte & 0x0f) as usize]));
            }
        }
    }
    encoded
}

fn form_pairs(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| format!("{}={}", form_component(name), form_component(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn build_upload_init_packet(input: &UploadInitInput<'_>) -> Result<UploadInitPacket> {
    if input.user_id.is_empty()
        || !input.user_id.bytes().all(|byte| byte.is_ascii_digit())
        || input.user_key.is_empty()
        || !input.user_key.is_ascii()
        || input.file_sha1.len() != 40
        || !input.file_sha1.bytes().all(|byte| byte.is_ascii_hexdigit())
        || input.filename.is_empty()
        || input.target_folder_id.is_empty()
        || !input
            .target_folder_id
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        bail!("115 Torrent 上传初始化参数无效");
    }

    let file_sha1 = input.file_sha1.to_ascii_uppercase();
    let target = format!("U_1_{}", input.target_folder_id);
    let timestamp = u32::try_from(input.timestamp).context("上传时间戳超出协议范围")?;
    let mut fields = BTreeMap::from([
        ("appid".to_string(), "0".to_string()),
        ("appversion".to_string(), BT_APP_VERSION.to_string()),
        ("fileid".to_string(), file_sha1.clone()),
        ("filename".to_string(), input.filename.to_string()),
        ("filesize".to_string(), input.file_size.to_string()),
        ("path".to_string(), String::new()),
        ("sign_key".to_string(), input.sign_key.to_string()),
        ("sign_val".to_string(), input.sign_val.to_string()),
        ("target".to_string(), target.clone()),
        ("t".to_string(), timestamp.to_string()),
        ("topupload".to_string(), "true".to_string()),
        ("userid".to_string(), input.user_id.to_string()),
        ("userkey".to_string(), input.user_key.to_string()),
    ]);

    let signature_seed = format!("{}{}{}0", input.user_id, file_sha1, target);
    let signature_seed_hex = to_hex(&sha1_bytes(signature_seed.as_bytes()));
    let mut signature = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    signature.update(input.user_key.as_bytes());
    signature.update(signature_seed_hex.as_bytes());
    signature.update(b"000000");
    fields.insert("sig".into(), hex_upper(signature.finish().as_ref()));

    let mut user_id_md5 = Md5::new();
    user_id_md5.update(input.user_id.as_bytes());
    let user_id_md5_hex = to_hex(&user_id_md5.finalize());
    let mut upload_token = Md5::new();
    upload_token.update(UPLOAD_MD5_SALT);
    upload_token.update(format!(
        "{}{}{}{}{}{}",
        file_sha1, input.file_size, input.sign_key, input.sign_val, input.user_id, timestamp
    ));
    upload_token.update(user_id_md5_hex.as_bytes());
    upload_token.update(BT_APP_VERSION.as_bytes());
    fields.insert("token".into(), to_hex(&upload_token.finalize()));

    let plaintext = form_pairs(&fields);
    let body = cbc::Encryptor::<Aes128>::new_from_slices(&UPLOAD_AES_KEY, &UPLOAD_AES_IV)
        .map_err(|_| anyhow!("115 上传 AES 参数无效"))?
        .encrypt_padded_vec_mut::<Pkcs7>(plaintext.as_bytes());
    Ok(UploadInitPacket {
        k_ec: encode_upload_ecdh_token(u64::from(timestamp))?,
        body,
    })
}

fn decrypt_upload_init_body(body: &[u8]) -> Result<Vec<u8>> {
    if body.is_empty() || body.len() % 16 != 0 {
        bail!("115 上传初始化响应 AES 数据长度无效");
    }
    cbc::Decryptor::<Aes128>::new_from_slices(&UPLOAD_AES_KEY, &UPLOAD_AES_IV)
        .map_err(|_| anyhow!("115 上传 AES 参数无效"))?
        .decrypt_padded_vec_mut::<Pkcs7>(body)
        .map_err(|_| anyhow!("115 上传初始化响应 AES 解密失败"))
}

fn decompress_upload_response_lz4(input: &[u8]) -> Result<Vec<u8>> {
    let mut offset = 0usize;
    let mut output = Vec::new();
    let mut blocks = 0usize;
    while offset < input.len() {
        if input.len() - offset < 2 {
            bail!("115 上传初始化响应 LZ4 分块头截断");
        }
        let compressed_size = usize::from(u16::from_le_bytes([input[offset], input[offset + 1]]));
        offset += 2;
        if compressed_size == 0 {
            if offset != input.len() {
                bail!("115 上传初始化响应 LZ4 结束标记后存在多余数据");
            }
            break;
        }
        let end = offset
            .checked_add(compressed_size)
            .ok_or_else(|| anyhow!("115 上传初始化响应 LZ4 长度溢出"))?;
        let compressed = input
            .get(offset..end)
            .ok_or_else(|| anyhow!("115 上传初始化响应 LZ4 分块截断"))?;
        let block = lz4_flex::block::decompress(compressed, UPLOAD_RESPONSE_BLOCK_SIZE)
            .context("115 上传初始化响应 LZ4 解压失败")?;
        if output.len().saturating_add(block.len()) > UPLOAD_RESPONSE_MAX_DECOMPRESSED {
            bail!("115 上传初始化响应解压后超出大小限制");
        }
        output.extend_from_slice(&block);
        offset = end;
        blocks += 1;
        if blocks > 4096 {
            bail!("115 上传初始化响应 LZ4 分块数量超出限制");
        }
    }
    if output.is_empty() {
        bail!("115 上传初始化响应 LZ4 内容为空");
    }
    Ok(output)
}

fn decode_upload_init_response(body: &[u8]) -> Result<serde_json::Value> {
    if body.len() > UPLOAD_RESPONSE_MAX_BYTES {
        bail!("115 上传初始化响应超出大小限制");
    }
    let decrypted = decrypt_upload_init_body(body)?;
    let json = decompress_upload_response_lz4(&decrypted)?;
    serde_json::from_slice(&json).context("解析 115 上传初始化响应 JSON 失败")
}

#[cfg(test)]
fn parse_upload_form(encoded: &[u8]) -> Result<BTreeMap<String, String>> {
    let text = std::str::from_utf8(encoded).context("上传表单编码不是 UTF-8")?;
    let mut fields = BTreeMap::new();
    for field in text.split('&').filter(|field| !field.is_empty()) {
        let (name, value) = field
            .split_once('=')
            .ok_or_else(|| anyhow!("上传表单字段格式无效"))?;
        let name = name.replace('+', " ");
        let value = value.replace('+', " ");
        let name = percent_encoding::percent_decode_str(&name)
            .decode_utf8()
            .context("上传表单字段名编码无效")?
            .into_owned();
        let value = percent_encoding::percent_decode_str(&value)
            .decode_utf8()
            .context("上传表单字段值编码无效")?
            .into_owned();
        if fields.insert(name, value).is_some() {
            bail!("上传表单存在重复字段");
        }
    }
    Ok(fields)
}

fn validate_wanted_indices(indices: &[u32], file_count: usize) -> Result<String> {
    if indices.is_empty() {
        bail!("至少选择一个 Torrent 文件");
    }
    let mut previous = None;
    for index in indices {
        if *index as usize >= file_count || previous.is_some_and(|p| p >= *index) {
            bail!("wanted 必须是有序、不重复且在文件清单范围内的零基索引");
        }
        previous = Some(*index);
    }
    Ok(indices
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(","))
}

fn build_ssp_bt_payload_json(
    info_hash: &str,
    wanted: &str,
    savepath: &str,
    folder_id: &str,
) -> String {
    // Keep the field names and string coercion of the pinned current SSP wrapper.
    format!(
        "{{\"info_hash\":{},\"wanted\":{},\"savepath\":{},\"wp_path_id\":{},\"ac\":\"add_task_bt\",\"app_ver\":{}}}",
        serde_json::to_string(info_hash).expect("serialize info_hash"),
        serde_json::to_string(wanted).expect("serialize wanted"),
        serde_json::to_string(savepath).expect("serialize savepath"),
        serde_json::to_string(folder_id).expect("serialize folder id"),
        serde_json::to_string(BT_APP_VERSION).expect("serialize app version"),
    )
}

fn build_ssp_bt_form_body(payload_json: &str) -> String {
    // Match the pinned p115cipher SSP wrapper, including its fixed 0x02 RSA
    // padding. The encrypted request body is never logged.
    let encrypted = super::m115_encode_ssp(payload_json);
    format!("data={}", form_component(&encrypted))
}

#[cfg(test)]
fn build_ssp_bt_form_data(payload_json: &str) -> String {
    // p115cipher's default SSP transform uses a fixed 0x02 PKCS#1 pad. The
    // existing transform's test-pad value 1 produces that byte after its
    // non-zero conversion. This helper is test-only; request bodies are never logged.
    super::m115_encode_with(payload_json, &[1])
}

fn select_exact_folder(candidates: &[FolderCandidate], name: &str) -> Result<FolderCandidate> {
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.is_dir && candidate.name == name);
    let found = matches
        .next()
        .cloned()
        .ok_or_else(|| anyhow!("目标目录缺失：{name}"))?;
    if matches.next().is_some()
        || found.id.is_empty()
        || !found.id.bytes().all(|byte| byte.is_ascii_digit())
    {
        bail!("目标目录名称不唯一或目录 ID 为空：{name}");
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    const MOCK_COOKIE: &str = "UID=12345_mock; SEID=unchanged";
    const MOCK_INFO_HASH: &str = "11d1d5c5e4ec0a7eae563fe0a6c3673283bac3ac";
    const MOCK_TORRENT: &[u8] = b"d4:infod6:lengthi3e4:name4:testee";
    static TEST_TORRENT_DIR_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Debug)]
    struct CapturedRequest {
        method: String,
        target: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    #[derive(Clone, Copy, Default)]
    struct MockBehavior {
        drop_upload_init_response: bool,
        upload_init_status: Option<u16>,
        task_list_status: Option<u16>,
        task_folder_id: Option<&'static str>,
        omit_task_folder: bool,
        multiple_matching_tasks: bool,
        submit_status: Option<u16>,
        submit_rejected_with_code: bool,
        drop_submit_response: bool,
        make_task_visible_after_submit: bool,
        first_folder_request_is_waf: bool,
        task_count_override: Option<u64>,
        task_page_count_override: Option<u64>,
    }

    struct MockState {
        requests: Mutex<Vec<CapturedRequest>>,
        task_visible: AtomicBool,
        upload_init_count: AtomicUsize,
        upload_object_count: AtomicUsize,
        submit_count: AtomicUsize,
        behavior: MockBehavior,
    }

    struct MockHttpServer {
        endpoints: BtEndpoints,
        state: Arc<MockState>,
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl MockHttpServer {
        fn start(behavior: MockBehavior) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.set_nonblocking(true).unwrap();
            let address = listener.local_addr().unwrap();
            let base = format!("http://{address}");
            let state = Arc::new(MockState {
                requests: Mutex::new(Vec::new()),
                task_visible: AtomicBool::new(false),
                upload_init_count: AtomicUsize::new(0),
                upload_object_count: AtomicUsize::new(0),
                submit_count: AtomicUsize::new(0),
                behavior,
            });
            let stop = Arc::new(AtomicBool::new(false));
            let server_state = Arc::clone(&state);
            let server_stop = Arc::clone(&stop);
            let server_thread = thread::spawn(move || {
                while !server_stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream.set_nonblocking(false).expect(
                                "accepted mock streams must support bounded blocking reads",
                            );
                            let Some(request) = read_mock_request(&mut stream) else {
                                continue;
                            };
                            let response = mock_response(&request, &server_state);
                            server_state.requests.lock().unwrap().push(request);
                            if let Some((status, body)) = response {
                                write_mock_response(&mut stream, status, &body);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                        }
                        Err(_) => break,
                    }
                }
            });
            Self {
                endpoints: BtEndpoints {
                    files_list: format!("{base}/files"),
                    upload_info: format!("{base}/files/uploadinfo"),
                    upload_init: format!("{base}/4.0/initupload.php"),
                    upload_token: format!("{base}/3.0/gettoken.php"),
                    upload_object_base: format!("{base}/"),
                    task_list: format!("{base}/web/"),
                    torrent_info: format!("{base}/"),
                    submit_bt: format!("{base}/lixianssp/"),
                },
                state,
                stop,
                thread: Some(server_thread),
            }
        }

        fn requests(&self) -> Vec<CapturedRequest> {
            self.state.requests.lock().unwrap().clone()
        }
    }

    impl Drop for MockHttpServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    struct TestTorrentDir(std::path::PathBuf);

    impl TestTorrentDir {
        fn new() -> Self {
            let root = loop {
                let sequence = TEST_TORRENT_DIR_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                let candidate = std::env::temp_dir().join(format!(
                    "rch_cookie_bt_http_mock_{}_{}",
                    std::process::id(),
                    sequence
                ));
                match std::fs::create_dir(&candidate) {
                    Ok(()) => break candidate,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                    Err(error) => panic!("create isolated mock Torrent directory: {error}"),
                }
            };
            std::fs::write(root.join("one.torrent"), MOCK_TORRENT).unwrap();
            Self(root)
        }

        fn path(&self) -> String {
            self.0.to_string_lossy().into_owned()
        }
    }

    impl Drop for TestTorrentDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn test_bt_client(server: &MockHttpServer) -> Cloud115WebClient {
        let mut client = Cloud115WebClient::new(MOCK_COOKIE, "0").unwrap();
        client.bt_strict_client = reqwest::blocking::Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(3))
            .connect_timeout(Duration::from_secs(1))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();
        client.bt_endpoints = server.endpoints.clone();
        client.gate = crate::source::RateGate::fixed_interval("115web.test", 0.0);
        client
    }

    fn test_target() -> Cloud115BtFolderSelection {
        Cloud115BtFolderSelection {
            id: "20".to_string(),
            parent_id: "10".to_string(),
            name: "EH订阅".to_string(),
            path: "/漫画/EH订阅".to_string(),
        }
    }

    fn read_mock_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
        stream.set_read_timeout(Some(Duration::from_secs(3))).ok()?;
        let mut header = Vec::new();
        let mut tail = [0u8; 4];
        loop {
            let mut byte = [0u8; 1];
            stream.read_exact(&mut byte).ok()?;
            header.push(byte[0]);
            tail.rotate_left(1);
            tail[3] = byte[0];
            if tail == *b"\r\n\r\n" {
                break;
            }
            if header.len() > 64 * 1024 {
                return None;
            }
        }
        let header_text = String::from_utf8(header).ok()?;
        let mut lines = header_text.split("\r\n");
        let mut first = lines.next()?.split_whitespace();
        let method = first.next()?.to_string();
        let target = first.next()?.to_string();
        let mut headers = HashMap::new();
        for line in lines.filter(|line| !line.is_empty()) {
            if let Some((name, value)) = line.split_once(':') {
                headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
            }
        }
        let length = headers
            .get("content-length")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).ok()?;
        Some(CapturedRequest {
            method,
            target,
            headers,
            body,
        })
    }

    fn mock_response(request: &CapturedRequest, state: &MockState) -> Option<(u16, Vec<u8>)> {
        let url = reqwest::Url::parse(&format!("http://localhost{}", request.target)).ok()?;
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let path = url.path();
        if request.method == "GET"
            && path == "/web/"
            && query.get("ac").map(String::as_str) == Some("task_lists")
        {
            if let Some(status) = state.behavior.task_list_status {
                return Some((status, b"mock task-list status".to_vec()));
            }
        }
        if state.behavior.first_folder_request_is_waf
            && path == "/files"
            && state.requests.lock().unwrap().is_empty()
        {
            return Some((405, b"challenge".to_vec()));
        }
        let json = match (request.method.as_str(), path) {
            ("GET", "/files") if path == "/files" => match query.get("cid").map(String::as_str) {
                Some("0") => serde_json::json!({
                    "state": true, "code": 0, "errno": 0, "count": 3,
                    "data": [
                        {"fid":"", "cid":"10", "n":"漫画", "s":"0", "t":"", "pc":""},
                        {"fid":"", "cid":"30", "n":"自定义目录", "s":"0", "t":"", "pc":""},
                        {"fid":"file-1", "cid":"", "n":"ignore.cbz", "s":"123", "t":"", "pc":"pick-file"}
                    ]
                }),
                Some("10") => serde_json::json!({
                    "state": true, "code": 0, "errno": 0, "count": 1,
                    "data": [{"fid":"", "cid":"20", "n":"EH订阅", "s":"0", "t":"", "pc":""}]
                }),
                Some("20") => serde_json::json!({
                    "state": true, "code": 0, "errno": 0, "count": 0, "data": []
                }),
                Some("30") => serde_json::json!({
                    "state": true, "code": 0, "errno": 0, "count": 0, "data": []
                }),
                _ => return Some((400, b"unexpected folder id".to_vec())),
            },
            ("GET", "/web/") if query.get("ac").map(String::as_str) == Some("task_lists") => {
                let visible = state.task_visible.load(Ordering::SeqCst);
                let count = state.behavior.task_count_override.unwrap_or(if visible {
                    if state.behavior.multiple_matching_tasks {
                        2
                    } else {
                        1
                    }
                } else {
                    0
                });
                let page_count = state
                    .behavior
                    .task_page_count_override
                    .unwrap_or(if visible {
                        1
                    } else if count == 0 {
                        0
                    } else {
                        1
                    });
                let tasks = if visible {
                    let mut task = serde_json::json!({"info_hash": MOCK_INFO_HASH, "status": 12, "status_text": "downloading", "progress": 0.25});
                    if !state.behavior.omit_task_folder {
                        task["wp_path_id"] =
                            serde_json::json!(state.behavior.task_folder_id.unwrap_or("20"));
                    }
                    if state.behavior.multiple_matching_tasks {
                        let mut other = task.clone();
                        other["wp_path_id"] = serde_json::json!("30");
                        serde_json::json!([task, other])
                    } else {
                        serde_json::json!([task])
                    }
                } else {
                    serde_json::json!([])
                };
                serde_json::json!({
                    "state": true,
                    "count": count,
                    "page_count": page_count,
                    "tasks": tasks
                })
            }
            ("GET", "/files/uploadinfo") => serde_json::json!({
                "state": true,
                "data": {"user_id": "12345", "userkey": "mock-userkey"}
            }),
            ("POST", "/4.0/initupload.php") => {
                state.upload_init_count.fetch_add(1, Ordering::SeqCst);
                if let Some(status) = state.behavior.upload_init_status {
                    return Some((status, b"mock upload init status".to_vec()));
                }
                if state.behavior.drop_upload_init_response {
                    return None;
                }
                let json = serde_json::json!({
                    "state": true,
                    "data": {
                        "status": 1,
                        "bucket": "fhnfile",
                        "object": "object-key",
                        "pickcode": "mock-pickcode",
                        "callback": {"callback": "callback-json", "callback_var": "callback-var"}
                    }
                });
                return Some((200, encrypted_upload_init_response(&json)));
            }
            ("GET", "/3.0/gettoken.php") => serde_json::json!({
                "AccessKeyId": "mock-access",
                "AccessKeySecret": "mock-secret",
                "SecurityToken": "mock-security"
            }),
            ("PUT", "/object-key") => {
                state.upload_object_count.fetch_add(1, Ordering::SeqCst);
                if request.body != MOCK_TORRENT
                    || request.headers.contains_key("cookie")
                    || !request.headers.contains_key("authorization")
                    || request.headers.get("content-type").map(String::as_str)
                        != Some("application/octet-stream")
                {
                    return Some((400, b"invalid OSS request".to_vec()));
                }
                return Some((200, b"{}".to_vec()));
            }
            ("GET", "/") if query.get("ac").map(String::as_str) == Some("torrent") => {
                serde_json::json!({
                    "state": true,
                    "data": {
                        "info_hash": MOCK_INFO_HASH,
                        "torrent_filelist_web": [{"path":"test", "size":3}]
                    }
                })
            }
            ("POST", "/lixianssp/") => {
                state.submit_count.fetch_add(1, Ordering::SeqCst);
                if let Some(status) = state.behavior.submit_status {
                    return Some((status, b"mock submit status".to_vec()));
                }
                if state.behavior.submit_rejected_with_code {
                    return Some((
                        200,
                        br#"{"state":false,"errno":1234,"error":"UID=secret; SEID=secret"}"#
                            .to_vec(),
                    ));
                }
                if state.behavior.make_task_visible_after_submit {
                    state.task_visible.store(true, Ordering::SeqCst);
                }
                if state.behavior.drop_submit_response {
                    return None;
                }
                return Some((200, br#"{"state":true}"#.to_vec()));
            }
            _ => return Some((404, b"unexpected endpoint".to_vec())),
        };
        Some((200, serde_json::to_vec(&json).unwrap()))
    }

    fn encrypted_upload_init_response(value: &serde_json::Value) -> Vec<u8> {
        let json = serde_json::to_vec(value).unwrap();
        let compressed = lz4_flex::block::compress(&json);
        let mut framed = Vec::new();
        framed.extend_from_slice(&(compressed.len() as u16).to_le_bytes());
        framed.extend_from_slice(&compressed);
        framed.extend_from_slice(&0u16.to_le_bytes());
        cbc::Encryptor::<Aes128>::new_from_slices(&UPLOAD_AES_KEY, &UPLOAD_AES_IV)
            .unwrap()
            .encrypt_padded_vec_mut::<Pkcs7>(&framed)
    }

    fn write_mock_response(stream: &mut TcpStream, status: u16, body: &[u8]) {
        let label = match status {
            200 => "OK",
            302 => "Found",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            _ => "Mock",
        };
        let header = format!(
            "HTTP/1.1 {status} {label}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\nSet-Cookie: UID=99999_changed; Path=/\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(header.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    }

    #[test]
    fn parses_v1_infohash_and_single_file_manifest() {
        let bytes = b"d4:infod6:lengthi3e4:name4:testee";
        let parsed = parse_torrent_bytes(bytes).unwrap();
        let expected = ring::digest::digest(
            &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
            b"d6:lengthi3e4:name4:teste",
        );
        assert_eq!(parsed.info_hash, to_hex(expected.as_ref()));
        assert_eq!(parsed.files.len(), 1);
        assert_eq!(parsed.files[0].index, 0);
        assert_eq!(parsed.files[0].path, "test");
        assert_eq!(parsed.files[0].size, 3);
        assert_eq!(
            parsed.torrent_sha1,
            to_hex(ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, bytes,).as_ref())
        );
    }

    #[test]
    fn rejects_noncanonical_or_unsafe_torrent_file_lists() {
        assert!(parse_torrent_bytes(b"d4:infod4:name4:test6:lengthi3eee").is_err());
        let traversal = b"d4:infod5:filesld6:lengthi3e4:pathl2:..4:evil eee4:name4:root ee";
        assert!(parse_torrent_bytes(traversal).is_err());
    }

    #[test]
    fn wanted_indices_are_zero_based_unique_and_in_range() {
        assert_eq!(validate_wanted_indices(&[0, 2], 3).unwrap(), "0,2");
        assert!(validate_wanted_indices(&[], 3).is_err());
        assert!(validate_wanted_indices(&[3], 3).is_err());
        assert!(validate_wanted_indices(&[1, 1], 3).is_err());
    }

    #[test]
    fn ssp_bt_payload_uses_current_encrypted_contract_fields() {
        let json = build_ssp_bt_payload_json(
            "0123456789012345678901234567890123456789",
            "0,2",
            "",
            "12345",
        );
        assert_eq!(
            json,
            r#"{"info_hash":"0123456789012345678901234567890123456789","wanted":"0,2","savepath":"","wp_path_id":"12345","ac":"add_task_bt","app_ver":"36.2.28"}"#
        );
        let form_data = build_ssp_bt_form_data(&json);
        assert!(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                form_data.as_bytes(),
            )
            .unwrap()
            .len()
                % 128
                == 0
        );
    }

    #[test]
    fn ssp_bt_wire_body_has_only_p115cipher_encoded_data_without_clear_fields() {
        let json = build_ssp_bt_payload_json(
            "0123456789012345678901234567890123456789",
            "0,2",
            "",
            "12345",
        );
        let body = build_ssp_bt_form_body(&json);
        assert!(body.starts_with("data="));
        assert!(!body.contains("info_hash="));
        assert!(!body.contains("wanted="));
        let encoded = decode_form_field(&body, "data").unwrap();
        assert!(!encoded.is_empty());
        assert!(!encoded.contains("0123456789012345678901234567890123456789"));
        assert_eq!(encoded, build_ssp_bt_form_data(&json));
        let ciphertext = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            encoded.as_bytes(),
        )
        .unwrap();
        assert_eq!(ciphertext.len() % 128, 0);
    }

    #[test]
    fn ssp_bt_rsa_wrapper_matches_pinned_p115cipher_default_encoding() {
        let json = build_ssp_bt_payload_json(
            "0123456789012345678901234567890123456789",
            "0,2",
            "",
            "12345",
        );
        let form_body = build_ssp_bt_form_body(&json);
        let encoded = decode_form_field(&form_body, "data").unwrap();
        let expected = super::super::m115_encode_with(&json, &[1]);
        assert_eq!(encoded, expected);
        // Independently generated from p115cipher at
        // 856e4f8f09ce034534bd458f06d3dee4474a1698, including fixed 0x02 RSA
        // padding and its default all-zero 16-byte random key.
        let digest = ring::digest::digest(&ring::digest::SHA256, encoded.as_bytes());
        assert_eq!(
            to_hex(digest.as_ref()),
            "662dfb79adffcffb8c869af53f56179399ca79dabe3b3bd118154ee9fa0c9488"
        );
    }

    #[test]
    fn upload_init_packet_keeps_file_sha1_distinct_from_infohash() {
        let input = UploadInitInput {
            user_id: "12345",
            user_key: "user-key-test",
            file_sha1: "abcdef0123456789abcdef0123456789abcdef01",
            filename: "测试 seed.torrent",
            file_size: 321,
            target_folder_id: "7890",
            sign_key: "",
            sign_val: "",
            timestamp: 1_760_000_000,
        };
        let packet = build_upload_init_packet(&input).unwrap();
        let encrypted = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            packet.k_ec.as_bytes(),
        )
        .unwrap();
        assert_eq!(encrypted.len(), 48);
        let decoded = decrypt_upload_init_body(&packet.body).unwrap();
        let fields = parse_upload_form(&decoded).unwrap();
        assert_eq!(
            fields.get("fileid").map(String::as_str),
            Some("ABCDEF0123456789ABCDEF0123456789ABCDEF01")
        );
        assert_eq!(
            fields.get("filename").map(String::as_str),
            Some("测试 seed.torrent")
        );
        assert_eq!(fields.get("filesize").map(String::as_str), Some("321"));
        assert_eq!(fields.get("target").map(String::as_str), Some("U_1_7890"));
        assert!(!fields.contains_key("info_hash"));
        assert!(fields.contains_key("sig"));
        assert!(fields.contains_key("token"));
    }

    #[test]
    fn upload_init_response_lz4_frame_rejects_truncated_or_oversized_blocks() {
        assert!(decompress_upload_response_lz4(&[4, 0, 0x10, b'{']).is_err());
        assert!(decompress_upload_response_lz4(&[0xff, 0xff, 0x10]).is_err());
    }

    fn decode_form_field(body: &str, expected_name: &str) -> Result<String> {
        let (name, value) = body
            .split_once('=')
            .ok_or_else(|| anyhow!("missing form field"))?;
        if name != expected_name {
            bail!("unexpected form field");
        }
        let bytes = percent_encoding::percent_decode_str(value)
            .decode_utf8()
            .context("invalid percent encoded form value")?;
        Ok(bytes.into_owned())
    }

    #[test]
    fn folder_lookup_requires_one_exact_directory_match() {
        let rows = vec![
            FolderCandidate {
                name: "漫画".into(),
                id: "1".into(),
                is_dir: true,
            },
            FolderCandidate {
                name: "漫画".into(),
                id: "2".into(),
                is_dir: false,
            },
        ];
        assert_eq!(select_exact_folder(&rows, "漫画").unwrap().id, "1");
        let ambiguous = vec![
            FolderCandidate {
                name: "漫画".into(),
                id: "1".into(),
                is_dir: true,
            },
            FolderCandidate {
                name: "漫画".into(),
                id: "3".into(),
                is_dir: true,
            },
        ];
        assert!(select_exact_folder(&ambiguous, "漫画").is_err());
        assert!(select_exact_folder(&rows, "漫画 ").is_err());
    }

    #[test]
    fn task_list_duplicate_check_requires_complete_contract_fields() {
        let matching = serde_json::json!({
            "state": true,
            "count": 1,
            "page_count": 1,
            "tasks": [{"info_hash": "0123456789012345678901234567890123456789"}]
        });
        assert!(
            !parse_task_list_page(&matching, "0123456789012345678901234567890123456789")
                .unwrap()
                .matched_tasks
                .is_empty()
        );

        let empty = serde_json::json!({
            "state": true,
            "count": 0,
            "page_count": 0,
            "tasks": []
        });
        assert!(
            parse_task_list_page(&empty, "0123456789012345678901234567890123456789")
                .unwrap()
                .matched_tasks
                .is_empty()
        );

        let incomplete = serde_json::json!({"state": true, "tasks": []});
        assert!(parse_task_list_page(&incomplete, "hash").is_err());
        let malformed_item = serde_json::json!({
            "state": true,
            "count": 1,
            "page_count": 1,
            "tasks": [{}]
        });
        assert!(parse_task_list_page(&malformed_item, "hash").is_err());
    }

    #[test]
    fn upload_identity_must_match_the_existing_cookie_account_without_exposing_user_key() {
        let parsed = parse_upload_identity(
            &serde_json::json!({
                "state": true,
                "data": { "user_id": "12345", "userkey": "secret-test-key" }
            }),
            "12345",
        )
        .unwrap();
        assert_eq!(parsed.user_id, "12345");
        assert_eq!(parsed.user_key, "secret-test-key");
        assert!(parse_upload_identity(
            &serde_json::json!({
                "state": true,
                "data": { "user_id": "54321", "userkey": "secret-test-key" }
            }),
            "12345",
        )
        .is_err());
        assert!(parse_upload_identity(
            &serde_json::json!({ "state": true, "data": { "user_id": "12345" } }),
            "12345",
        )
        .is_err());
    }

    #[test]
    fn upload_init_status_requires_pickcode_and_never_confuses_sha_values() {
        let info = parse_upload_init_result(&serde_json::json!({
            "state": true,
            "pickcode": "pck-uploaded",
            "data": {
                "status": 1,
                "bucket": "fhnfile",
                "object": "object-key",
                "callback": {
                    "callback": "callback-body",
                    "callback_var": "callback-vars"
                }
            }
        }))
        .unwrap();
        assert_eq!(info.status, 1);
        assert_eq!(info.pick_code, "pck-uploaded");
        assert_eq!(info.object, "object-key");
        assert_eq!(info.bucket, "fhnfile");

        let reused = parse_upload_init_result(&serde_json::json!({
            "state": true,
            "pickcode": "pck-reused",
            "data": { "status": 2 }
        }))
        .unwrap();
        assert_eq!(reused.status, 2);
        assert_eq!(reused.pick_code, "pck-reused");

        assert!(parse_upload_init_result(&serde_json::json!({
            "state": true,
            "pickcode": "",
            "data": { "status": 2 }
        }))
        .is_err());
    }

    #[test]
    fn remote_torrent_parse_must_match_local_infohash_and_ordered_files() {
        let expected_hash = "0123456789012345678901234567890123456789";
        let files = parse_remote_torrent_parse(
            &serde_json::json!({
                "state": true,
                "info_hash": expected_hash,
                "torrent_filelist_web": [
                    { "name": "001.cbz", "size": 123 },
                    { "name": "readme.nfo", "size": 7 }
                ]
            }),
            expected_hash,
            &[
                Cloud115BtFile {
                    index: 0,
                    path: "001.cbz".into(),
                    size: 123,
                },
                Cloud115BtFile {
                    index: 1,
                    path: "readme.nfo".into(),
                    size: 7,
                },
            ],
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        assert!(parse_remote_torrent_parse(
            &serde_json::json!({
                "state": true,
                "info_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "torrent_filelist_web": [{ "name": "001.cbz", "size": 123 }]
            }),
            expected_hash,
            &[Cloud115BtFile {
                index: 0,
                path: "001.cbz".into(),
                size: 123
            }],
        )
        .is_err());
    }

    #[test]
    fn oss_authorization_and_upload_url_are_https_and_reject_untrusted_hosts() {
        let token = OssToken {
            access_key_id: "test-access".into(),
            access_key_secret: "test-secret".into(),
            security_token: "test-security-token".into(),
        };
        let authorization = build_oss_authorization(
            &token,
            "PUT",
            "fhnfile",
            "/object-key",
            "Thu, 01 Jan 1970 00:00:00 GMT",
            "application/octet-stream",
            "callback",
            "callback-var",
        )
        .unwrap();
        assert!(authorization.starts_with("OSS test-access:"));
        assert!(!authorization.contains("test-secret"));
        assert_eq!(
            build_oss_upload_url("fhnfile", "object-key").unwrap(),
            "https://fhnfile.oss-cn-shenzhen.aliyuncs.com/object-key"
        );
        assert!(build_oss_upload_url("attacker.example", "object-key").is_err());
        assert!(build_oss_upload_url("fhnfile", "../outside").is_err());

        let canonical = concat!(
            "PUT\n\napplication/octet-stream\n",
            "Thu, 01 Jan 1970 00:00:00 GMT\n",
            "x-oss-callback:Y2FsbGJhY2s=\n",
            "x-oss-callback-var:Y2FsbGJhY2stdmFy\n",
            "x-oss-security-token:test-security-token\n",
            "/fhnfile/object-key"
        );
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, b"test-secret");
        let expected = format!(
            "OSS test-access:{}",
            base64::engine::general_purpose::STANDARD
                .encode(ring::hmac::sign(&key, canonical.as_bytes()).as_ref())
        );
        assert_eq!(authorization, expected);
    }

    #[test]
    fn mock_http_upload_parse_and_single_bt_submit_use_isolated_encrypted_transport() {
        let server = MockHttpServer::start(MockBehavior {
            make_task_visible_after_submit: true,
            ..MockBehavior::default()
        });
        // A matching task in the task list must not block an explicitly
        // confirmed submission. 115 owns duplicate handling at submit time.
        server.state.task_visible.store(true, Ordering::SeqCst);
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let uploaded = client
            .bt_upload_and_parse(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .unwrap();
        assert_eq!(uploaded.state, "PARSED");
        assert_eq!(uploaded.target_folder_id, "20");
        assert_eq!(uploaded.info_hash, MOCK_INFO_HASH);
        assert_eq!(uploaded.files.len(), 1);
        assert_eq!(uploaded.files[0].path, "test");
        assert!(!uploaded.upload_reused);

        let submitted = client
            .bt_submit_once(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                &[0],
            )
            .unwrap();
        assert_eq!(submitted.state, "ACCEPTED");
        let repeated = client
            .bt_submit_once(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                &[0],
            )
            .unwrap();
        assert_eq!(repeated.state, "UNKNOWN_REMOTE_STATE");

        let requests_before_status = server.requests();
        assert!(requests_before_status
            .iter()
            .all(|request| { !request.target.contains("ac=task_lists") }));

        let status = client
            .bt_task_status(MOCK_INFO_HASH, &test_target())
            .unwrap();
        assert_eq!(status.state, "found");
        assert_eq!(status.status_text, "downloading");
        assert_eq!(status.percent_done, Some(0.25));
        assert_eq!(client.cookie(), MOCK_COOKIE);

        let requests = server.requests();
        assert_eq!(server.state.upload_init_count.load(Ordering::SeqCst), 1);
        assert_eq!(server.state.upload_object_count.load(Ordering::SeqCst), 1);
        assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 1);
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.target.contains("ac=task_lists"))
                .count(),
            1,
            "only the explicit post-submit status query may scan tasks"
        );
        assert_eq!(
            requests
                .iter()
                .filter(
                    |request| request.method == "POST" && request.target.starts_with("/lixianssp/")
                )
                .count(),
            1
        );

        let upload_info = requests
            .iter()
            .find(|request| request.target.starts_with("/files/uploadinfo"))
            .unwrap();
        assert_eq!(
            upload_info.headers.get("cookie").map(String::as_str),
            Some(MOCK_COOKIE)
        );
        let token = requests
            .iter()
            .find(|request| request.target.starts_with("/3.0/gettoken.php"))
            .unwrap();
        assert!(!token.headers.contains_key("cookie"));

        let init = requests
            .iter()
            .find(|request| request.target.starts_with("/4.0/initupload.php"))
            .unwrap();
        let init_text = String::from_utf8_lossy(&init.body);
        assert!(!init_text.contains("mock-userkey"));
        assert!(!init_text.contains(&uploaded.torrent_sha1));

        let oss = requests
            .iter()
            .find(|request| request.method == "PUT" && request.target.starts_with("/object-key"))
            .unwrap();
        assert_eq!(oss.body, MOCK_TORRENT);
        assert!(!oss.headers.contains_key("cookie"));
        assert!(oss
            .headers
            .get("authorization")
            .unwrap()
            .starts_with("OSS mock-access:"));
        assert_eq!(
            oss.headers.get("content-type").map(String::as_str),
            Some("application/octet-stream")
        );

        let submit = requests
            .iter()
            .find(|request| request.method == "POST" && request.target.starts_with("/lixianssp/"))
            .unwrap();
        assert_eq!(
            submit.headers.get("cookie").map(String::as_str),
            Some(MOCK_COOKIE)
        );
        let body = String::from_utf8(submit.body.clone()).unwrap();
        assert!(body.starts_with("data="));
        assert!(!body.contains("info_hash="));
        assert!(!body.contains("wanted="));
        assert!(!body.contains(MOCK_INFO_HASH));
        let encrypted = decode_form_field(&body, "data").unwrap();
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(encrypted)
            .unwrap();
        assert!(!ciphertext.is_empty());
        assert_eq!(ciphertext.len() % 128, 0);
        assert!(requests.iter().all(|request| {
            !request.headers.contains_key("cookie") || request.headers["cookie"] == MOCK_COOKIE
        }));
    }

    #[test]
    fn bt_task_snapshot_reads_all_tasks_in_one_bounded_read_only_cycle() {
        let server = MockHttpServer::start(MockBehavior::default());
        server.state.task_visible.store(true, Ordering::SeqCst);
        let client = test_bt_client(&server);
        let snapshot = client.bt_task_snapshot().unwrap();
        assert_eq!(snapshot.account_id, "12345");
        assert_eq!(snapshot.state, "ok");
        assert!(snapshot.complete);
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(snapshot.tasks[0].info_hash, MOCK_INFO_HASH);
        assert_eq!(snapshot.tasks[0].status_code, Some(12));
        assert_eq!(snapshot.tasks[0].percent_done, Some(0.25));
        let requests = server.requests();
        assert_eq!(
            requests
                .iter()
                .filter(|request| request.target.contains("ac=task_lists"))
                .count(),
            1
        );
        assert_eq!(server.state.upload_init_count.load(Ordering::SeqCst), 0);
        assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn snapshot_preserves_matching_hashes_in_distinct_destinations() {
        let server = MockHttpServer::start(MockBehavior {
            multiple_matching_tasks: true,
            ..MockBehavior::default()
        });
        server.state.task_visible.store(true, Ordering::SeqCst);
        let client = test_bt_client(&server);
        let snapshot = client.bt_task_snapshot().unwrap();
        assert!(snapshot.complete);
        assert_eq!(snapshot.tasks.len(), 2);
        assert_eq!(snapshot.tasks[0].target_folder_id.as_deref(), Some("20"));
        assert_eq!(snapshot.tasks[1].target_folder_id.as_deref(), Some("30"));
    }

    #[test]
    fn bt_task_snapshot_marks_over_budget_listing_partial_without_absence_claims() {
        let server = MockHttpServer::start(MockBehavior {
            task_count_override: Some(301),
            task_page_count_override: Some(11),
            ..MockBehavior::default()
        });
        server.state.task_visible.store(true, Ordering::SeqCst);
        let client = test_bt_client(&server);
        let snapshot = client.bt_task_snapshot().unwrap();
        assert_eq!(snapshot.state, "partial");
        assert!(!snapshot.complete);
        assert_eq!(
            snapshot.error_class.as_deref(),
            Some("query_budget_exhausted")
        );
        assert_eq!(snapshot.tasks.len(), 1);
        assert_eq!(
            server
                .requests()
                .iter()
                .filter(|request| request.target.contains("ac=task_lists"))
                .count(),
            1
        );
    }

    #[test]
    fn task_page_parser_returns_multiple_hashes_for_single_snapshot_matching() {
        let response = serde_json::json!({
            "state": true,
            "count": 2,
            "page_count": 1,
            "tasks": [
                {"info_hash": MOCK_INFO_HASH, "status": 12, "progress": 0.4},
                {"info_hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "status": 77}
            ]
        });
        let parsed = parse_task_list_page(&response, "").unwrap();
        assert_eq!(parsed.tasks.len(), 2);
        assert_eq!(parsed.tasks[0].info_hash, MOCK_INFO_HASH);
        assert_eq!(parsed.tasks[1].status_code, Some(77));
    }

    #[test]
    fn submit_rejection_reports_numeric_code_without_provider_secrets() {
        let server = MockHttpServer::start(MockBehavior {
            submit_rejected_with_code: true,
            ..MockBehavior::default()
        });
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);
        let result = client
            .bt_submit_once(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                &[0],
            )
            .unwrap();
        assert_eq!(result.state, "REJECTED");
        assert!(result.message.contains("1234"));
        assert!(!result.message.contains("secret"));
        assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn submit_waf_rate_limit_and_auth_failures_are_structured() {
        let dir = TestTorrentDir::new();
        for (http_status, expected_state) in [
            (405, "WAF_BLOCKED"),
            (429, "RATE_LIMITED"),
            (401, "AUTH_EXPIRED"),
            (403, "ACCESS_DENIED"),
        ] {
            let server = MockHttpServer::start(MockBehavior {
                submit_status: Some(http_status),
                ..MockBehavior::default()
            });
            let client = test_bt_client(&server);
            let result = client
                .bt_submit_once(
                    &dir.path(),
                    "one.torrent",
                    MOCK_INFO_HASH,
                    &test_target(),
                    &[0],
                )
                .unwrap();

            assert_eq!(result.state, expected_state);
            assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 1);
            assert_eq!(
                server
                    .requests()
                    .iter()
                    .filter(|request| request.target.contains("ac=task_lists"))
                    .count(),
                0,
                "a definitive gate response must not trigger reconciliation"
            );
        }
    }

    #[test]
    fn uncertain_upload_and_submit_are_not_reposted_in_the_same_session() {
        let upload_server = MockHttpServer::start(MockBehavior {
            drop_upload_init_response: true,
            ..MockBehavior::default()
        });
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&upload_server);
        let first = client
            .bt_upload_and_parse(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .unwrap();
        assert_eq!(first.state, "UNKNOWN_UPLOAD_RESULT");
        let second = client
            .bt_upload_and_parse(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .unwrap();
        assert_eq!(second.state, "UNKNOWN_UPLOAD_RESULT");
        assert_eq!(
            upload_server.state.upload_init_count.load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            upload_server
                .state
                .upload_object_count
                .load(Ordering::SeqCst),
            0
        );
        assert_eq!(client.cookie(), MOCK_COOKIE);

        let submit_server = MockHttpServer::start(MockBehavior {
            drop_submit_response: true,
            ..MockBehavior::default()
        });
        let submit_client = test_bt_client(&submit_server);
        let first = submit_client
            .bt_submit_once(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                &[0],
            )
            .unwrap();
        assert_eq!(first.state, "UNKNOWN_REMOTE_STATE");
        let second = submit_client
            .bt_submit_once(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                &[0],
            )
            .unwrap();
        assert_eq!(second.state, "UNKNOWN_REMOTE_STATE");
        let captured: Vec<_> = submit_server
            .requests()
            .iter()
            .map(|request| {
                format!(
                    "{} {}",
                    request.method,
                    request.target.split('?').next().unwrap_or("/")
                )
            })
            .collect();
        assert_eq!(
            submit_server.state.submit_count.load(Ordering::SeqCst),
            1,
            "captured mock requests: {captured:?}"
        );
        assert_eq!(submit_client.cookie(), MOCK_COOKIE);
    }

    #[test]
    fn upload_init_waf_and_rate_limit_are_structured_batch_stop_states() {
        let dir = TestTorrentDir::new();
        for (status, expected_state) in [(405, "WAF_BLOCKED"), (429, "RATE_LIMITED")] {
            let server = MockHttpServer::start(MockBehavior {
                upload_init_status: Some(status),
                ..MockBehavior::default()
            });
            let client = test_bt_client(&server);
            let result = client
                .bt_upload_and_parse(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
                .unwrap();

            assert_eq!(result.state, expected_state);
            assert_eq!(server.state.upload_init_count.load(Ordering::SeqCst), 1);
            assert_eq!(server.state.upload_object_count.load(Ordering::SeqCst), 0);
            assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn task_list_waf_and_rate_limit_return_structured_statuses() {
        for (http_status, expected_state) in [(405, "WAF_BLOCKED"), (429, "RATE_LIMITED")] {
            let server = MockHttpServer::start(MockBehavior {
                task_list_status: Some(http_status),
                ..MockBehavior::default()
            });
            let client = test_bt_client(&server);
            let result = client
                .bt_task_status(MOCK_INFO_HASH, &test_target())
                .unwrap();
            assert_eq!(result.state, expected_state);
            assert_eq!(result.status_code, Some(i64::from(http_status)));
            assert_eq!(server.state.upload_init_count.load(Ordering::SeqCst), 0);
            assert_eq!(server.state.submit_count.load(Ordering::SeqCst), 0);
        }
    }

    #[test]
    fn mock_waf_405_stops_before_any_upload_or_submit_request() {
        let server = MockHttpServer::start(MockBehavior {
            first_folder_request_is_waf: true,
            ..MockBehavior::default()
        });
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);
        assert!(client
            .bt_readonly_preflight(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .is_err());
        let request_count = server.requests().len();
        assert_eq!(request_count, 1);
        assert!(client
            .bt_readonly_preflight(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .is_err());
        assert_eq!(server.requests().len(), request_count);
        assert_eq!(client.cookie(), MOCK_COOKIE);
    }

    #[test]
    fn folder_picker_lists_only_directories_without_recording_open_time() {
        let server = MockHttpServer::start(MockBehavior::default());
        let client = test_bt_client(&server);

        let folders = client.bt_list_child_folders("0").unwrap();

        assert_eq!(
            folders
                .iter()
                .map(|folder| (folder.name.as_str(), folder.id.as_str()))
                .collect::<Vec<_>>(),
            vec![("漫画", "10"), ("自定义目录", "30")]
        );
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].method, "GET");
        let url = reqwest::Url::parse(&format!("http://localhost{}", requests[0].target)).unwrap();
        let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
        assert_eq!(query.get("cid").map(String::as_str), Some("0"));
        assert_eq!(query.get("record_open_time").map(String::as_str), Some("0"));
        assert_eq!(
            requests[0].headers.get("cookie").map(String::as_str),
            Some(MOCK_COOKIE)
        );
    }

    #[test]
    fn test_torrent_directories_are_unique_when_created_concurrently() {
        let workers: Vec<_> = (0..32)
            .map(|_| thread::spawn(TestTorrentDir::new))
            .collect();
        let directories: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        let unique_paths: std::collections::HashSet<_> =
            directories.iter().map(|directory| &directory.0).collect();

        assert_eq!(unique_paths.len(), directories.len());
        assert!(directories
            .iter()
            .all(|directory| directory.0.join("one.torrent").is_file()));
    }

    #[test]
    fn preflight_uses_the_user_selected_existing_folder() {
        let server = MockHttpServer::start(MockBehavior::default());
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client
            .bt_readonly_preflight(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &Cloud115BtFolderSelection {
                    id: "30".to_string(),
                    parent_id: "0".to_string(),
                    name: "自定义目录".to_string(),
                    path: "/自定义目录".to_string(),
                },
            )
            .unwrap();

        assert_eq!(result.target_folder_id, "30");
        assert_eq!(result.target_folder_path, "/自定义目录");
        assert_eq!(result.duplicate_status, "submit_first");
        let requests = server.requests();
        assert!(requests.iter().any(|request| {
            request.target.contains("cid=0") && request.target.contains("record_open_time=0")
        }));
        assert!(requests
            .iter()
            .all(|request| !request.target.contains("ac=task_lists")));
    }

    #[test]
    fn preflight_does_not_scan_task_pages_before_user_confirmed_submission() {
        let server = MockHttpServer::start(MockBehavior {
            task_count_override: Some(301),
            task_page_count_override: Some(11),
            ..MockBehavior::default()
        });
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client
            .bt_readonly_preflight(&dir.path(), "one.torrent", MOCK_INFO_HASH, &test_target())
            .unwrap();

        assert_eq!(result.duplicate_status, "submit_first");
        assert!(result.duplicate_detail.is_empty());
        assert!(server.requests().iter().all(|request| {
            request.method == "GET"
                && !request.target.contains("initupload")
                && !request.target.contains("lixianssp")
                && !request.target.contains("ac=task_lists")
        }));
    }

    #[test]
    fn upload_recovery_does_not_scan_task_list() {
        let server = MockHttpServer::start(MockBehavior {
            task_count_override: Some(301),
            task_page_count_override: Some(11),
            ..MockBehavior::default()
        });
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client
            .bt_confirm_upload_parse(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                &test_target(),
                Some("mock-pickcode".into()),
            )
            .unwrap();

        assert_eq!(result.state, "PARSED");
        assert!(server
            .requests()
            .iter()
            .all(|request| !request.target.contains("ac=task_lists")));
    }

    #[test]
    fn preflight_rejects_a_stale_or_mismatched_selected_folder_before_task_lookup() {
        let server = MockHttpServer::start(MockBehavior::default());
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client.bt_readonly_preflight(
            &dir.path(),
            "one.torrent",
            MOCK_INFO_HASH,
            &Cloud115BtFolderSelection {
                id: "99".to_string(),
                parent_id: "0".to_string(),
                name: "自定义目录".to_string(),
                path: "/自定义目录".to_string(),
            },
        );

        assert!(result.is_err());
        assert!(server
            .requests()
            .iter()
            .all(|request| !request.target.contains("ac=task_lists")));
    }

    #[test]
    fn local_torrent_is_deleted_only_after_same_account_remote_task_is_confirmed() {
        let server = MockHttpServer::start(MockBehavior::default());
        server.state.task_visible.store(true, Ordering::SeqCst);
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        client
            .bt_delete_local_torrent_if_remote_task_found(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                "12345",
                &test_target(),
            )
            .unwrap();

        assert!(!dir.0.join("one.torrent").exists());
        assert!(server.requests().iter().all(|request| {
            request.method == "GET"
                && !request.target.contains("upload")
                && !request.target.contains("lixianssp")
        }));
        assert_eq!(client.cookie(), MOCK_COOKIE);
    }

    #[test]
    fn matching_hash_without_matching_remote_destination_is_not_proof_for_cleanup() {
        for behavior in [
            MockBehavior {
                omit_task_folder: true,
                ..MockBehavior::default()
            },
            MockBehavior {
                task_folder_id: Some("30"),
                ..MockBehavior::default()
            },
        ] {
            let server = MockHttpServer::start(behavior);
            server.state.task_visible.store(true, Ordering::SeqCst);
            let dir = TestTorrentDir::new();
            let client = test_bt_client(&server);
            let status = client
                .bt_task_status(MOCK_INFO_HASH, &test_target())
                .unwrap();
            assert_eq!(status.state, "unknown");
            assert_ne!(status.target_folder_id, "20");
            let deleted = client.bt_delete_local_torrent_if_remote_task_found(
                &dir.path(),
                "one.torrent",
                MOCK_INFO_HASH,
                "12345",
                &test_target(),
            );
            assert!(deleted.is_err());
            assert!(dir.0.join("one.torrent").is_file());
        }
    }

    #[test]
    fn local_torrent_is_retained_when_remote_task_is_not_confirmed() {
        let server = MockHttpServer::start(MockBehavior::default());
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client.bt_delete_local_torrent_if_remote_task_found(
            &dir.path(),
            "one.torrent",
            MOCK_INFO_HASH,
            "12345",
            &test_target(),
        );

        assert!(result.is_err());
        assert!(dir.0.join("one.torrent").is_file());
    }

    #[test]
    fn local_torrent_is_retained_when_cookie_account_changed() {
        let server = MockHttpServer::start(MockBehavior::default());
        server.state.task_visible.store(true, Ordering::SeqCst);
        let dir = TestTorrentDir::new();
        let client = test_bt_client(&server);

        let result = client.bt_delete_local_torrent_if_remote_task_found(
            &dir.path(),
            "one.torrent",
            MOCK_INFO_HASH,
            "99999",
            &test_target(),
        );

        assert!(result.is_err());
        assert!(dir.0.join("one.torrent").is_file());
        assert!(server.requests().is_empty());
    }
}
