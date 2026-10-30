//! Model classification: LEGACY_WEB_PROTOCOL_MODEL.
//! Test-only historical clear-form Cookie protocol model; it does not model
//! current SSP wire traffic. Its URL guard is loopback-only so this helper
//! cannot send requests to a 115 host.

use reqwest::blocking::Client;
use ring::digest::{digest, SHA1_FOR_LEGACY_USE_ONLY};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const TEST_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) RCH protocol test";
const MULTIPART_BOUNDARY: &str = "rch-cookie-torrent-fixture";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentIdentity {
    pub file_sha1: String,
    pub info_hash: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignTime {
    pub sign: String,
    pub time: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadedTorrent {
    pub file_id: String,
    pub pickcode: String,
    pub sha1: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TorrentFile {
    pub name: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedTorrent {
    pub info_hash: String,
    pub torrent_name: String,
    pub files: Vec<TorrentFile>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SubmitOutcome {
    Accepted,
    AlreadyPresent,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProtocolError {
    WafBlocked,
    RemoteUploadStateUnknown(String),
    RemoteStateUnknown(String),
    Rejected,
    InvalidResponse,
    InvalidTorrent,
    InvalidWantedIndex,
    RequestFailed,
}

pub struct WebCookieProtocol {
    origin: String,
    client: Client,
    cookie: String,
    now_millis: u64,
    unknown_uploads: Mutex<HashSet<String>>,
    unknown_submissions: Mutex<HashSet<String>>,
}

impl WebCookieProtocol {
    pub fn new(
        origin: impl AsRef<str>,
        cookie: &str,
        now_millis: u64,
    ) -> Result<Self, ProtocolError> {
        let parsed =
            reqwest::Url::parse(origin.as_ref()).map_err(|_| ProtocolError::InvalidResponse)?;
        let loopback = parsed
            .host_str()
            .and_then(|host| host.parse::<IpAddr>().ok())
            .is_some_and(|ip| ip.is_loopback());
        if !loopback
            || parsed.scheme() != "http"
            || parsed.port().is_none()
            || !matches!(parsed.path(), "" | "/")
            || parsed.query().is_some()
            || parsed.fragment().is_some()
            || parsed.username() != ""
            || parsed.password().is_some()
        {
            return Err(ProtocolError::InvalidResponse);
        }
        let host = parsed.host_str().ok_or(ProtocolError::InvalidResponse)?;
        let port = parsed.port().ok_or(ProtocolError::InvalidResponse)?;
        let client = Client::builder()
            .timeout(Duration::from_secs(2))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| ProtocolError::RequestFailed)?;

        Ok(Self {
            origin: format!("http://{host}:{port}"),
            client,
            cookie: cookie.to_owned(),
            now_millis,
            unknown_uploads: Mutex::new(HashSet::new()),
            unknown_submissions: Mutex::new(HashSet::new()),
        })
    }

    pub fn fetch_sign_time(&self) -> Result<SignTime, ProtocolError> {
        let target = format!("/?ct=offline&ac=space&_={}", self.now_millis);
        let json = self.request_json("GET", &target, None)?;
        let sign = json
            .get("sign")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        let time = json
            .get("time")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        Ok(SignTime {
            sign: sign.to_owned(),
            time: time.to_owned(),
        })
    }

    pub fn upload_torrent(
        &self,
        bytes: &[u8],
        filename: &str,
        upload_folder_id: &str,
    ) -> Result<UploadedTorrent, ProtocolError> {
        let identity = torrent_identity(bytes)?;
        if self
            .unknown_uploads
            .lock()
            .map_err(|_| ProtocolError::RequestFailed)?
            .contains(&identity.file_sha1)
        {
            return Err(ProtocolError::RemoteUploadStateUnknown(identity.file_sha1));
        }
        let mut body = Vec::new();
        append_multipart_text(&mut body, "Filename", filename);
        append_multipart_text(&mut body, "target", &format!("U_1_{upload_folder_id}"));
        body.extend_from_slice(
            format!(
                "--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"Filedata\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
        append_multipart_text(&mut body, "Upload", "Submit Query");
        body.extend_from_slice(format!("--{MULTIPART_BOUNDARY}--\r\n").as_bytes());

        let upload_json = match self.request_json(
            "POST",
            "/mock-upload",
            Some((
                format!("multipart/form-data; boundary={MULTIPART_BOUNDARY}"),
                body,
            )),
        ) {
            Ok(json) => json,
            Err(ProtocolError::WafBlocked) => return Err(ProtocolError::WafBlocked),
            Err(_) => {
                self.unknown_uploads
                    .lock()
                    .map_err(|_| ProtocolError::RequestFailed)?
                    .insert(identity.file_sha1.clone());
                return Err(ProtocolError::RemoteUploadStateUnknown(identity.file_sha1));
            }
        };
        let file_id = upload_json
            .pointer("/data/file_id")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;

        // The legacy upload response supplies file_id; a follow-up file lookup
        // yields the pickcode and whole-file SHA-1 used by Torrent parsing.
        let metadata = self.request_json(
            "POST",
            "/webapi/file",
            Some((
                "application/x-www-form-urlencoded".to_owned(),
                form_body(&[("file_id", file_id)]),
            )),
        )?;
        let data = metadata
            .get("data")
            .and_then(Value::as_array)
            .and_then(|items| items.first())
            .ok_or(ProtocolError::InvalidResponse)?;
        let pickcode = data
            .get("pickcode")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        let remote_sha1 = data
            .get("sha")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        if !remote_sha1.eq_ignore_ascii_case(&identity.file_sha1) {
            return Err(ProtocolError::InvalidResponse);
        }

        Ok(UploadedTorrent {
            file_id: file_id.to_owned(),
            pickcode: pickcode.to_owned(),
            sha1: remote_sha1.to_ascii_uppercase(),
        })
    }

    pub fn parse_torrent(
        &self,
        uploaded: &UploadedTorrent,
        user_id: &str,
        sign_time: &SignTime,
        expected: &TorrentIdentity,
    ) -> Result<ParsedTorrent, ProtocolError> {
        let form = form_body(&[
            ("pickcode", &uploaded.pickcode),
            ("sha", &uploaded.sha1),
            ("uid", user_id),
            ("sign", &sign_time.sign),
            ("time", &sign_time.time),
        ]);
        let json = self.request_json(
            "POST",
            "/lixian/?ct=lixian&ac=torrent",
            Some(("application/x-www-form-urlencoded".to_owned(), form)),
        )?;
        let info_hash = json
            .get("info_hash")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        if !info_hash.eq_ignore_ascii_case(&expected.info_hash) {
            return Err(ProtocolError::InvalidTorrent);
        }
        let torrent_filelist = json
            .get("torrent_filelist_web")
            .and_then(Value::as_array)
            .ok_or(ProtocolError::InvalidResponse)?;
        let mut files = Vec::with_capacity(torrent_filelist.len());
        for file in torrent_filelist {
            let name = file
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ProtocolError::InvalidResponse)?;
            let size = file
                .get("size")
                .and_then(Value::as_u64)
                .ok_or(ProtocolError::InvalidResponse)?;
            files.push(TorrentFile {
                name: name.to_owned(),
                size,
            });
        }
        let torrent_name = json
            .get("torrent_name")
            .and_then(Value::as_str)
            .ok_or(ProtocolError::InvalidResponse)?;
        Ok(ParsedTorrent {
            info_hash: info_hash.to_ascii_lowercase(),
            torrent_name: torrent_name.to_owned(),
            files,
        })
    }

    pub fn submit_once(
        &self,
        torrent: &ParsedTorrent,
        user_id: &str,
        sign_time: &SignTime,
        savepath: &str,
        wanted_indices: &[usize],
    ) -> Result<SubmitOutcome, ProtocolError> {
        wanted_form(wanted_indices, Some(torrent.files.len()))?;
        self.submit_hash_once(
            &torrent.info_hash,
            user_id,
            &sign_time.sign,
            &sign_time.time,
            savepath,
            wanted_indices,
        )
    }

    pub fn submit_hash_once(
        &self,
        info_hash: &str,
        user_id: &str,
        sign: &str,
        time: &str,
        savepath: &str,
        wanted_indices: &[usize],
    ) -> Result<SubmitOutcome, ProtocolError> {
        if self
            .unknown_submissions
            .lock()
            .map_err(|_| ProtocolError::RequestFailed)?
            .contains(info_hash)
        {
            return self.recover_unknown(info_hash, user_id, sign, time);
        }

        if self.task_exists(info_hash, user_id, sign, time)? {
            return Ok(SubmitOutcome::AlreadyPresent);
        }
        let wanted = wanted_form(wanted_indices, None)?;
        let form = form_body(&[
            ("info_hash", info_hash),
            ("wanted", &wanted),
            ("savepath", savepath),
            ("uid", user_id),
            ("sign", sign),
            ("time", time),
        ]);
        let response = self.request_json(
            "POST",
            "/lixian/?ct=lixian&ac=add_task_bt",
            Some(("application/x-www-form-urlencoded".to_owned(), form)),
        );
        match response {
            Ok(json) if json.get("state").and_then(Value::as_bool) == Some(true) => {
                Ok(SubmitOutcome::Accepted)
            }
            Ok(_) => Err(ProtocolError::Rejected),
            Err(ProtocolError::WafBlocked) => Err(ProtocolError::WafBlocked),
            Err(_) => {
                self.unknown_submissions
                    .lock()
                    .map_err(|_| ProtocolError::RequestFailed)?
                    .insert(info_hash.to_owned());
                Err(ProtocolError::RemoteStateUnknown(info_hash.to_owned()))
            }
        }
    }

    pub fn recover_unknown(
        &self,
        info_hash: &str,
        user_id: &str,
        sign: &str,
        time: &str,
    ) -> Result<SubmitOutcome, ProtocolError> {
        let exists = self.task_exists(info_hash, user_id, sign, time)?;
        if exists {
            self.unknown_submissions
                .lock()
                .map_err(|_| ProtocolError::RequestFailed)?
                .remove(info_hash);
            Ok(SubmitOutcome::AlreadyPresent)
        } else {
            Err(ProtocolError::RemoteStateUnknown(info_hash.to_owned()))
        }
    }

    fn task_exists(
        &self,
        info_hash: &str,
        user_id: &str,
        sign: &str,
        time: &str,
    ) -> Result<bool, ProtocolError> {
        let form = form_body(&[
            ("page", "1"),
            ("uid", user_id),
            ("sign", sign),
            ("time", time),
        ]);
        let json = self.request_json(
            "POST",
            "/lixian/?ct=lixian&ac=task_lists",
            Some(("application/x-www-form-urlencoded".to_owned(), form)),
        )?;
        let tasks = json
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or(ProtocolError::InvalidResponse)?;
        Ok(tasks
            .iter()
            .any(|task| task.get("info_hash").and_then(Value::as_str) == Some(info_hash)))
    }

    fn request_json(
        &self,
        method: &str,
        target: &str,
        body: Option<(String, Vec<u8>)>,
    ) -> Result<Value, ProtocolError> {
        let url = format!("{}{target}", self.origin);
        let mut request = self
            .client
            .request(
                method.parse().map_err(|_| ProtocolError::RequestFailed)?,
                url,
            )
            .header(reqwest::header::COOKIE, &self.cookie)
            .header(reqwest::header::USER_AGENT, TEST_USER_AGENT);
        if let Some((content_type, body)) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body);
        }
        let response = request.send().map_err(|_| ProtocolError::RequestFailed)?;
        if response.status().as_u16() == 405 {
            return Err(ProtocolError::WafBlocked);
        }
        if !response.status().is_success() {
            return Err(ProtocolError::Rejected);
        }
        response.json().map_err(|_| ProtocolError::InvalidResponse)
    }
}

fn wanted_form(indices: &[usize], file_count: Option<usize>) -> Result<String, ProtocolError> {
    if indices
        .iter()
        .any(|index| file_count.is_some_and(|count| *index >= count))
    {
        return Err(ProtocolError::InvalidWantedIndex);
    }
    Ok(indices
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(","))
}

fn form_body(fields: &[(&str, &str)]) -> Vec<u8> {
    fields
        .iter()
        .map(|(key, value)| format!("{}={}", form_encode(key), form_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
        .into_bytes()
}

fn form_encode(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else if byte == b' ' {
            encoded.push('+');
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn append_multipart_text(body: &mut Vec<u8>, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{MULTIPART_BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

pub fn torrent_identity(bytes: &[u8]) -> Result<TorrentIdentity, ProtocolError> {
    let info = top_level_info_slice(bytes)?;
    Ok(TorrentIdentity {
        file_sha1: sha1_hex(bytes),
        info_hash: sha1_hex(info),
    })
}

fn sha1_hex(bytes: &[u8]) -> String {
    digest(&SHA1_FOR_LEGACY_USE_ONLY, bytes)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn top_level_info_slice(bytes: &[u8]) -> Result<&[u8], ProtocolError> {
    if bytes.first() != Some(&b'd') {
        return Err(ProtocolError::InvalidTorrent);
    }
    let mut cursor = 1;
    let mut info_span = None;
    loop {
        if bytes.get(cursor) == Some(&b'e') {
            cursor += 1;
            break;
        }
        let key = read_bencode_bytes(bytes, &mut cursor)?;
        let start = cursor;
        skip_bencode_value(bytes, &mut cursor)?;
        if key == b"info" {
            if info_span.replace((start, cursor)).is_some() {
                return Err(ProtocolError::InvalidTorrent);
            }
        }
    }
    if cursor != bytes.len() {
        return Err(ProtocolError::InvalidTorrent);
    }
    let (start, end) = info_span.ok_or(ProtocolError::InvalidTorrent)?;
    Ok(&bytes[start..end])
}

fn read_bencode_bytes<'a>(bytes: &'a [u8], cursor: &mut usize) -> Result<&'a [u8], ProtocolError> {
    let length_start = *cursor;
    while bytes.get(*cursor).is_some_and(u8::is_ascii_digit) {
        *cursor += 1;
    }
    if bytes.get(*cursor) != Some(&b':') || *cursor == length_start {
        return Err(ProtocolError::InvalidTorrent);
    }
    let length = std::str::from_utf8(&bytes[length_start..*cursor])
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or(ProtocolError::InvalidTorrent)?;
    *cursor += 1;
    let end = cursor
        .checked_add(length)
        .ok_or(ProtocolError::InvalidTorrent)?;
    if end > bytes.len() {
        return Err(ProtocolError::InvalidTorrent);
    }
    let result = &bytes[*cursor..end];
    *cursor = end;
    Ok(result)
}

fn skip_bencode_value(bytes: &[u8], cursor: &mut usize) -> Result<(), ProtocolError> {
    match bytes.get(*cursor).copied() {
        Some(b'i') => {
            *cursor += 1;
            let end = bytes[*cursor..]
                .iter()
                .position(|byte| *byte == b'e')
                .ok_or(ProtocolError::InvalidTorrent)?;
            *cursor += end + 1;
            Ok(())
        }
        Some(b'l') => {
            *cursor += 1;
            while bytes.get(*cursor) != Some(&b'e') {
                skip_bencode_value(bytes, cursor)?;
            }
            *cursor += 1;
            Ok(())
        }
        Some(b'd') => {
            *cursor += 1;
            while bytes.get(*cursor) != Some(&b'e') {
                read_bencode_bytes(bytes, cursor)?;
                skip_bencode_value(bytes, cursor)?;
            }
            *cursor += 1;
            Ok(())
        }
        Some(byte) if byte.is_ascii_digit() => {
            read_bencode_bytes(bytes, cursor)?;
            Ok(())
        }
        _ => Err(ProtocolError::InvalidTorrent),
    }
}

pub fn fixture_torrent() -> Vec<u8> {
    let mut info = b"d5:filesl".to_vec();
    info.extend_from_slice(b"d6:lengthi4e4:pathl7:001.jpgee");
    info.extend_from_slice(b"d6:lengthi2e4:pathl10:readme.nfoee");
    info.extend_from_slice(b"e4:name6:sample12:piece lengthi16384e6:pieces20:");
    info.extend_from_slice(b"abcdefghijklmnopqrst");
    info.extend_from_slice(b"e");

    let announce = b"https://tracker.invalid/announce";
    let mut torrent = b"d8:announce".to_vec();
    torrent.extend_from_slice(format!("{}:", announce.len()).as_bytes());
    torrent.extend_from_slice(announce);
    torrent.extend_from_slice(b"4:info");
    torrent.extend_from_slice(&info);
    torrent.push(b'e');
    torrent
}

#[derive(Clone)]
pub enum MockReply {
    Json { status: u16, body: String },
    CloseWithoutResponse,
    DelayWithoutResponse(Duration),
}

impl MockReply {
    pub fn json(status: u16, body: impl Into<String>) -> Self {
        Self::Json {
            status,
            body: body.into(),
        }
    }

    pub fn close_without_response() -> Self {
        Self::CloseWithoutResponse
    }

    pub fn delay_without_response(delay: Duration) -> Self {
        Self::DelayWithoutResponse(delay)
    }
}

#[derive(Clone, Debug)]
pub struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl CapturedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub struct MockServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start(replies: Vec<MockReply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback mock");
        listener
            .set_nonblocking(true)
            .expect("set mock nonblocking");
        let addr = listener.local_addr().expect("mock local address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread = thread::spawn(move || {
            for reply in replies {
                let mut stream = loop {
                    if thread_stop.load(Ordering::SeqCst) {
                        return;
                    }
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => return,
                    }
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                let Some(request) = read_request(&mut stream) else {
                    return;
                };
                thread_requests
                    .lock()
                    .expect("mock request lock")
                    .push(request);
                match reply {
                    MockReply::CloseWithoutResponse => {}
                    MockReply::DelayWithoutResponse(delay) => thread::sleep(delay),
                    MockReply::Json { status, body } => {
                        write_json_response(&mut stream, status, &body)
                    }
                }
            }
        });
        Self {
            addr,
            requests,
            stop,
            thread: Some(thread),
        }
    }

    pub fn addr(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn finish(mut self) -> Vec<CapturedRequest> {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        self.requests.lock().expect("mock request lock").clone()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn read_request(stream: &mut TcpStream) -> Option<CapturedRequest> {
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..count]);
        if let Some(index) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break index + 4;
        }
        if raw.len() > 1024 * 1024 {
            return None;
        }
    };
    let header_text = std::str::from_utf8(&raw[..header_end]).ok()?;
    let mut lines = header_text.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_owned();
    let target = request_line.next()?.to_owned();
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_owned());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    while raw.len() < header_end + content_length {
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        raw.extend_from_slice(&chunk[..count]);
    }
    Some(CapturedRequest {
        method,
        target,
        headers,
        body: raw[header_end..header_end + content_length].to_vec(),
    })
}

fn write_json_response(stream: &mut TcpStream, status: u16, body: &str) {
    let reason = match status {
        200 => "OK",
        405 => "Method Not Allowed",
        _ => "Mock Response",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes());
}
