//! Shared integration-test support: a tiny in-process WebDAV server and a
//! throw-away SD-card root.
//!
//! The mock speaks just enough HTTP/1.1 + WebDAV for linkSlot: GET, PUT,
//! MKCOL, MOVE, DELETE. Requests are percent-decoded before storage so tests
//! can use logical UTF-8 paths, while the raw request target is recorded so
//! tests can assert per-segment URL encoding.

#![allow(dead_code)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use linkslot::cache::{self, Cache, RemoteRef};
use linkslot::config::Config;
use linkslot::hash;
use linkslot::meta::Meta;
use linkslot::net::NetMode;
use linkslot::output::{Outcome, Reporter};
use linkslot::playtime;
use linkslot::sync::{self, Ctx, PullOptions, PushOptions, SyncOptions};
use linkslot::unit::{Layout, Unit, UnitSpec, LAYOUT_NAME};
use linkslot::webdav::{base64_encode, encode_segment};

// ---------------------------------------------------------------------------
// Temp directory
// ---------------------------------------------------------------------------

pub struct TestDir {
    path: PathBuf,
}

impl TestDir {
    pub fn new(tag: &str) -> TestDir {
        let path = std::env::temp_dir().join(format!(
            "linkslot-it-{tag}-{}-{}",
            std::process::id(),
            linkslot::util::unique_suffix()
        ));
        std::fs::create_dir_all(&path).unwrap();
        TestDir { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

// ---------------------------------------------------------------------------
// Percent decoding (mock side only; production code encodes)
// ---------------------------------------------------------------------------

pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(hex) = hex {
                if let Ok(value) = u8::from_str_radix(hex, 16) {
                    out.push(value);
                    index += 3;
                    continue;
                }
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

// ---------------------------------------------------------------------------
// Mock WebDAV server
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct RequestRecord {
    pub method: String,
    /// Raw request target exactly as sent (percent encoded).
    pub raw_path: String,
    /// Percent-decoded logical path.
    pub path: String,
    pub auth: Option<String>,
    pub destination: Option<String>,
}

#[derive(Clone, Debug)]
struct FailRule {
    method: String,
    path_contains: Option<String>,
    status: u16,
    remaining: usize,
}

#[derive(Default)]
pub struct DavState {
    pub files: BTreeMap<String, Vec<u8>>,
    pub dirs: BTreeSet<String>,
    pub requests: Vec<RequestRecord>,
    pub move_supported: bool,
    /// Expected `(user, password)`; `None` disables auth checks.
    pub auth: Option<(String, String)>,
    /// Paths whose GET body is corrupted (verification tests).
    pub corrupt: BTreeSet<String>,
    fail_rules: Vec<FailRule>,
}

impl DavState {
    fn new() -> DavState {
        DavState {
            files: BTreeMap::new(),
            dirs: BTreeSet::new(),
            requests: Vec::new(),
            move_supported: true,
            auth: None,
            corrupt: BTreeSet::new(),
            fail_rules: Vec::new(),
        }
    }

    fn parent(path: &str) -> String {
        match path.rfind('/') {
            Some(0) | None => "/".to_string(),
            Some(index) => path[..index].to_string(),
        }
    }

    fn ensure_parent_dirs(&mut self, path: &str) {
        let mut current = DavState::parent(path);
        while current != "/" && !current.is_empty() {
            self.dirs.insert(current.clone());
            current = DavState::parent(&current);
        }
        self.dirs.insert("/".to_string());
    }
}

pub struct MockDav {
    addr: SocketAddr,
    state: Arc<Mutex<DavState>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockDav {
    pub fn start() -> MockDav {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock dav");
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(Mutex::new(DavState::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let server_state = Arc::clone(&state);
        let server_stop = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if server_stop.load(Ordering::Relaxed) {
                    break;
                }
                match stream {
                    Ok(stream) => handle_connection(stream, &server_state),
                    Err(_) => break,
                }
            }
        });
        MockDav {
            addr,
            state,
            stop,
            handle: Some(handle),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn state(&self) -> Arc<Mutex<DavState>> {
        Arc::clone(&self.state)
    }

    pub fn set_move_supported(&self, supported: bool) {
        self.state.lock().unwrap().move_supported = supported;
    }

    pub fn set_auth(&self, user: &str, password: &str) {
        self.state.lock().unwrap().auth = Some((user.to_string(), password.to_string()));
    }

    pub fn fail_next(&self, method: &str, status: u16, times: usize) {
        self.state.lock().unwrap().fail_rules.push(FailRule {
            method: method.to_string(),
            path_contains: None,
            status,
            remaining: times,
        });
    }

    /// Fail the next `times` requests whose method matches and whose decoded
    /// path contains `path_contains` (used to break one specific file, e.g. the
    /// best-effort playtime PUT, without disturbing the save transfer).
    pub fn fail_path(&self, method: &str, path_contains: &str, status: u16, times: usize) {
        self.state.lock().unwrap().fail_rules.push(FailRule {
            method: method.to_string(),
            path_contains: Some(path_contains.to_string()),
            status,
            remaining: times,
        });
    }

    pub fn corrupt(&self, path: &str) {
        self.state.lock().unwrap().corrupt.insert(path.to_string());
    }

    /// Insert a file, creating all logical parent collections.
    pub fn insert_file(&self, path: &str, bytes: &[u8]) {
        let mut state = self.state.lock().unwrap();
        state.ensure_parent_dirs(path);
        state.files.insert(path.to_string(), bytes.to_vec());
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.state.lock().unwrap().files.get(path).cloned()
    }

    pub fn has_file(&self, path: &str) -> bool {
        self.state.lock().unwrap().files.contains_key(path)
    }

    pub fn has_dir(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        self.state.lock().unwrap().dirs.contains(path)
    }

    /// Logical collection paths (sorted).
    pub fn dir_names(&self) -> Vec<String> {
        self.state.lock().unwrap().dirs.iter().cloned().collect()
    }

    pub fn file_names(&self) -> Vec<String> {
        self.state.lock().unwrap().files.keys().cloned().collect()
    }

    pub fn requests(&self) -> Vec<RequestRecord> {
        self.state.lock().unwrap().requests.clone()
    }

    pub fn request_methods(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .map(|record| record.method.clone())
            .collect()
    }

    /// Percent-decoded paths of every request using `method`.
    pub fn request_paths(&self, method: &str) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .requests
            .iter()
            .filter(|record| record.method == method)
            .map(|record| record.path.clone())
            .collect()
    }

    /// Percent-decoded paths of every `PUT` request (backup uploads included).
    pub fn put_paths(&self) -> Vec<String> {
        self.request_paths("PUT")
    }

    pub fn get_paths(&self) -> Vec<String> {
        self.request_paths("GET")
    }

    /// Logical file paths below `prefix` (sorted).
    pub fn file_names_under(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .state
            .lock()
            .unwrap()
            .files
            .keys()
            .filter(|path| path.starts_with(prefix))
            .cloned()
            .collect();
        names.sort();
        names
    }

    /// Logical collection paths below `prefix` (sorted).
    pub fn dir_names_under(&self, prefix: &str) -> Vec<String> {
        let mut names: Vec<String> = self
            .state
            .lock()
            .unwrap()
            .dirs
            .iter()
            .filter(|path| path.starts_with(prefix))
            .cloned()
            .collect();
        names.sort();
        names
    }
}

impl Drop for MockDav {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, state: &Arc<Mutex<DavState>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
    let Some((method, target, headers, body)) = read_request(&mut stream) else {
        return;
    };
    let raw_path = target.clone();
    let path = decode_target(&target);
    let auth = headers.get("authorization").cloned();
    let destination = headers.get("destination").map(|dest| decode_target(dest));

    let mut state = state.lock().unwrap();
    state.requests.push(RequestRecord {
        method: method.clone(),
        raw_path: raw_path.clone(),
        path: path.clone(),
        auth: auth.clone(),
        destination: destination.clone(),
    });

    // Injected failures (retry tests).
    if let Some(rule) = state.fail_rules.iter_mut().find(|rule| {
        rule.method == method
            && rule.remaining > 0
            && rule
                .path_contains
                .as_deref()
                .is_none_or(|needle| path.contains(needle))
    }) {
        rule.remaining -= 1;
        let status = rule.status;
        drop(state);
        respond(&mut stream, status, b"", &[]);
        return;
    }

    // Auth check.
    if let Some((user, password)) = state.auth.clone() {
        let expected = format!(
            "Basic {}",
            base64_encode(format!("{user}:{password}").as_bytes())
        );
        if auth.as_deref() != Some(expected.as_str()) {
            drop(state);
            respond(
                &mut stream,
                401,
                b"unauthorized",
                &[("WWW-Authenticate", "Basic realm=\"linkslot\"")],
            );
            return;
        }
    }

    match method.as_str() {
        "GET" => {
            if let Some(mut body) = state.files.get(&path).cloned() {
                if state.corrupt.contains(&path) {
                    body.push(0xff);
                }
                drop(state);
                respond(&mut stream, 200, &body, &[]);
            } else {
                drop(state);
                respond(&mut stream, 404, b"not found", &[]);
            }
        }
        "PUT" => {
            let parent = DavState::parent(&path);
            if parent != "/" && !state.dirs.contains(&parent) {
                drop(state);
                respond(&mut stream, 409, b"missing collection", &[]);
                return;
            }
            state.ensure_parent_dirs(&path);
            state.files.insert(path.clone(), body);
            drop(state);
            respond(&mut stream, 201, b"", &[]);
        }
        "MKCOL" => {
            if state.dirs.contains(&path) {
                drop(state);
                respond(&mut stream, 405, b"already exists", &[]);
                return;
            }
            let parent = DavState::parent(&path);
            if parent != "/" && !state.dirs.contains(&parent) {
                drop(state);
                respond(&mut stream, 409, b"missing parent", &[]);
                return;
            }
            state.dirs.insert(path.clone());
            drop(state);
            respond(&mut stream, 201, b"", &[]);
        }
        "DELETE" => {
            if state.files.remove(&path).is_some() {
                drop(state);
                respond(&mut stream, 204, b"", &[]);
                return;
            }
            let dir_path = path.trim_end_matches('/').to_string();
            let has_children = state
                .files
                .keys()
                .any(|file| file.starts_with(&format!("{dir_path}/")));
            if state.dirs.remove(&dir_path) && !has_children {
                drop(state);
                respond(&mut stream, 204, b"", &[]);
            } else {
                drop(state);
                respond(&mut stream, 404, b"not found", &[]);
            }
        }
        "MOVE" => {
            if !state.move_supported {
                drop(state);
                respond(&mut stream, 405, b"move not supported", &[]);
                return;
            }
            let Some(dest) = destination else {
                drop(state);
                respond(&mut stream, 400, b"missing destination", &[]);
                return;
            };
            let Some(body) = state.files.remove(&path) else {
                drop(state);
                respond(&mut stream, 404, b"source not found", &[]);
                return;
            };
            let parent = DavState::parent(&dest);
            if parent != "/" && !state.dirs.contains(&parent) {
                state.files.insert(path.clone(), body);
                drop(state);
                respond(&mut stream, 409, b"missing destination collection", &[]);
                return;
            }
            state.ensure_parent_dirs(&dest);
            state.files.insert(dest, body);
            drop(state);
            respond(&mut stream, 201, b"", &[]);
        }
        "HEAD" => {
            let exists = state.files.contains_key(&path);
            drop(state);
            respond(&mut stream, if exists { 200 } else { 404 }, b"", &[]);
        }
        "PROPFIND" => {
            let depth = headers.get("depth").map(String::as_str).unwrap_or("0");
            let body = propfind_body(&state, &raw_path, &path, depth);
            drop(state);
            match body {
                Some(body) => respond(
                    &mut stream,
                    207,
                    body.as_bytes(),
                    &[("Content-Type", "application/xml; charset=utf-8")],
                ),
                None => respond(&mut stream, 404, b"not found", &[]),
            }
        }
        _ => {
            drop(state);
            respond(&mut stream, 405, b"method not allowed", &[]);
        }
    }
}

/// Render a PROPFIND `207 Multi-Status` body for `path`.
///
/// `Depth: 0` returns only the resource itself; `Depth: 1` adds the direct
/// children of a collection. `None` means the resource does not exist (the
/// client maps HTTP 404 to `Ok(None)`).
fn propfind_body(state: &DavState, raw_path: &str, path: &str, depth: &str) -> Option<String> {
    let is_dir = state.dirs.contains(path);
    let file = state.files.get(path);
    if !is_dir && file.is_none() {
        return None;
    }
    let self_href = if is_dir {
        dir_href(raw_path)
    } else {
        raw_path.to_string()
    };
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">");
    push_dav_response(
        &mut xml,
        &self_href,
        is_dir,
        file.map_or(0, |bytes| bytes.len() as u64),
    );
    if depth == "1" && is_dir {
        let base = self_href.trim_end_matches('/').to_string();
        for (file_path, bytes) in &state.files {
            if DavState::parent(file_path) != path {
                continue;
            }
            if let Some(name) = file_path.rsplit('/').next() {
                let href = format!("{base}/{}", encode_segment(name));
                push_dav_response(&mut xml, &href, false, bytes.len() as u64);
            }
        }
        for child in &state.dirs {
            if child == path || DavState::parent(child) != path {
                continue;
            }
            if let Some(name) = child.rsplit('/').next().filter(|name| !name.is_empty()) {
                let href = format!("{base}/{}/", encode_segment(name));
                push_dav_response(&mut xml, &href, true, 0);
            }
        }
    }
    xml.push_str("</D:multistatus>");
    Some(xml)
}

fn dir_href(raw_path: &str) -> String {
    if raw_path.ends_with('/') {
        raw_path.to_string()
    } else {
        format!("{raw_path}/")
    }
}

fn push_dav_response(xml: &mut String, href: &str, is_dir: bool, size: u64) {
    xml.push_str("<D:response><D:href>");
    xml.push_str(&xml_escape(href));
    xml.push_str("</D:href><D:propstat><D:prop>");
    if is_dir {
        xml.push_str(
            "<D:resourcetype><D:collection/></D:resourcetype><D:getcontentlength>0</D:getcontentlength>",
        );
    } else {
        xml.push_str("<D:resourcetype/><D:getcontentlength>");
        xml.push_str(&size.to_string());
        xml.push_str("</D:getcontentlength>");
    }
    xml.push_str(
        "<D:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</D:getlastmodified></D:prop>\
         <D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>",
    );
}

fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

type ParsedRequest = (String, String, HashMap<String, String>, Vec<u8>);

fn read_request(stream: &mut TcpStream) -> Option<ParsedRequest> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(head_end) = find_bytes(&buffer, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
            let mut rest = buffer[head_end + 4..].to_vec();
            let mut lines = head.split("\r\n");
            let request_line = lines.next()?;
            let mut parts = request_line.split_whitespace();
            let method = parts.next()?.to_string();
            let target = parts.next()?.to_string();
            let mut headers = HashMap::new();
            for line in lines {
                if let Some((name, value)) = line.split_once(':') {
                    headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                }
            }
            let length: usize = headers
                .get("content-length")
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            while rest.len() < length {
                let read = stream.read(&mut chunk).ok()?;
                if read == 0 {
                    break;
                }
                rest.extend_from_slice(&chunk[..read]);
            }
            rest.truncate(length);
            return Some((method, target, headers, rest));
        }
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// Accept both origin-form (`/dav/x`) and absolute-form (`http://host/dav/x`).
fn decode_target(target: &str) -> String {
    let path = if let Some(rest) = target.strip_prefix("http://") {
        match rest.find('/') {
            Some(index) => &rest[index..],
            None => "/",
        }
    } else if let Some(rest) = target.strip_prefix("https://") {
        match rest.find('/') {
            Some(index) => &rest[index..],
            None => "/",
        }
    } else {
        target
    };
    let path = path.split('?').next().unwrap_or(path);
    let decoded = percent_decode(path);
    if decoded.len() > 1 {
        decoded.trim_end_matches('/').to_string()
    } else {
        decoded
    }
}

fn respond(stream: &mut TcpStream, status: u16, body: &[u8], headers: &[(&str, &str)]) {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "Status",
    };
    let mut head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

// ---------------------------------------------------------------------------
// SD-card + config harness
// ---------------------------------------------------------------------------

pub struct RunResult {
    pub outcome: Outcome,
    pub stdout: String,
    pub stderr: String,
}

pub struct Harness {
    pub dir: TestDir,
    pub root: PathBuf,
    pub dav: MockDav,
    pub config: Config,
    pub ctx: Ctx,
}

pub const GAME: &str = "合金弹头";

/// Fixed ROM every harness card gets at `Games/GBA/<GAME>.gba`. `Harness::unit`
/// is identified in the cloud by this file's full-file SHA-256.
pub const FIXTURE_ROM: &[u8] = b"linkSlot test ROM fixture v1";
/// Different bytes for the same game stem (different cloud unit).
pub const OTHER_ROM: &[u8] = b"linkSlot test ROM fixture v2, different bytes";

impl Harness {
    pub fn new(tag: &str) -> Harness {
        Harness::with_options(tag, 0, false)
    }

    pub fn with_options(tag: &str, retries: u32, keep_remote: bool) -> Harness {
        let dir = TestDir::new(tag);
        let root = dir.path().join("sd");
        std::fs::create_dir_all(&root).unwrap();
        let dav = MockDav::start();
        let config_path = root.join("linkSlot/Config/config.toml");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        let text = format!(
            r#"[webdav]
url = "{}"
remote_root = "/linkSlot"
username = "user"
password = "pass"

[device]
id = "rg34xxsp-deadbeef"

[sync]
timeout_secs = 10
retries = {retries}
keep_remote = {keep_remote}

[wifi]
manage = false
ssid = ""
psk = ""
timeout_secs = 5
"#,
            dav.url()
        );
        std::fs::write(&config_path, text).unwrap();
        let config = Config::load(&root, None).unwrap();
        let ctx = Ctx::new(root.clone()).with_net_mode(NetMode::Skip);
        // The card always has the fixed fixture ROM the CLI can auto-discover.
        let games = root.join("Games").join("GBA");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join(format!("{GAME}.gba")), FIXTURE_ROM).unwrap();
        Harness {
            dir,
            root,
            dav,
            config,
            ctx,
        }
    }

    pub fn unit(&self) -> Unit {
        self.unit_named(GAME)
    }

    /// Path of a fixture ROM under `Games/GBA/`.
    pub fn rom_path(&self, game: &str) -> PathBuf {
        self.root
            .join("Games")
            .join("GBA")
            .join(format!("{game}.gba"))
    }

    /// Write (or replace) `Games/GBA/<game>.gba`; returns the ROM path.
    pub fn write_rom(&self, game: &str, bytes: &[u8]) -> PathBuf {
        let path = self.rom_path(game);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// The unit for a custom game name, creating its ROM from [`FIXTURE_ROM`]
    /// when it does not exist yet. The ROM file is the source of truth for the
    /// hash, so overwriting it with [`Harness::write_rom`] changes the identity.
    pub fn unit_named(&self, game: &str) -> Unit {
        let path = self.rom_path(game);
        if !path.is_file() {
            self.write_rom(game, FIXTURE_ROM);
        }
        self.unit_for_rom(game, &path)
    }

    /// The unit for `game` with an explicitly written ROM payload.
    pub fn unit_with_rom(&self, game: &str, rom: &[u8]) -> Unit {
        let path = self.write_rom(game, rom);
        self.unit_for_rom(game, &path)
    }

    fn unit_for_rom(&self, game: &str, rom: &Path) -> Unit {
        let digest = hash::sha256_file(rom).unwrap();
        Unit::new("GBA", "mgba", game)
            .unwrap()
            .with_rom_sha256(&digest)
            .unwrap()
    }

    /// SHA-256 of the fixed fixture ROM (equals `unit().rom_sha256`).
    pub fn fixture_rom_digest(&self) -> String {
        hash::sha256_hex(FIXTURE_ROM)
    }

    pub fn unit_dir(&self, unit: &Unit) -> PathBuf {
        unit.local_dir(&self.root)
    }

    /// Logical remote path of a file inside the unit directory.
    pub fn remote_path(&self, unit: &Unit, name: &str) -> String {
        self.remote_path_unit(unit, name)
    }

    /// Logical remote path for any unit (game-name units included).
    pub fn remote_path_unit(&self, unit: &Unit, name: &str) -> String {
        format!("{}/{}", self.remote_unit_dir(unit), name)
    }

    pub fn remote_meta_path(&self, unit: &Unit) -> String {
        self.remote_path(unit, "meta.json")
    }

    fn remote_root(&self) -> String {
        self.config
            .webdav()
            .remote_root
            .trim_end_matches('/')
            .to_string()
    }

    /// Logical remote *game* directory of one unit:
    /// `/linkSlot/<platform>/<64 hex sha256>` (playtime.json lives here).
    pub fn remote_game_dir(&self, unit: &Unit) -> String {
        let mut path = self.remote_root();
        for segment in unit.remote_segments() {
            path.push('/');
            path.push_str(&segment);
        }
        path
    }

    /// Logical remote directory of the state unit:
    /// `/linkSlot/<platform>/<64 hex sha256>/state/<core>`.
    pub fn remote_unit_dir(&self, unit: &Unit) -> String {
        let mut path = self.remote_root();
        for segment in UnitSpec::state(unit, &self.root)
            .remote_segments(unit)
            .expect("harness unit carries a ROM hash")
        {
            path.push('/');
            path.push_str(&segment);
        }
        path
    }

    /// The pre-v0.0.10 game-name cloud path
    /// (`/linkSlot/GBA/mgba/<game>`). Old data must never be read, moved,
    /// overwritten or deleted by v0.0.10 syncs.
    pub fn remote_legacy_unit_dir(&self, unit: &Unit) -> String {
        format!(
            "{}/{}/{}/{}",
            self.remote_root(),
            unit.platform,
            unit.emulator,
            unit.game
        )
    }

    pub fn remote_legacy_path(&self, unit: &Unit, name: &str) -> String {
        format!("{}/{}", self.remote_legacy_unit_dir(unit), name)
    }

    // ---- battery save (.sav) ---------------------------------------------

    /// Local `Saves/GBA/<game>.sav` path.
    pub fn sav_path(&self, unit: &Unit) -> PathBuf {
        unit.local_sav_path(&self.root)
    }

    /// Local `Saves/GBA/<game>.srm` path.
    pub fn srm_path(&self, unit: &Unit) -> PathBuf {
        unit.local_srm_path(&self.root)
    }

    /// Write the local battery save (`ext` = `sav` / `srm`), creating `Saves/`.
    pub fn write_local_sav(&self, unit: &Unit, ext: &str, bytes: &[u8]) -> PathBuf {
        let path = self.sav_path(unit).with_extension(ext);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    pub fn local_sav(&self, unit: &Unit) -> Option<Vec<u8>> {
        std::fs::read(self.sav_path(unit)).ok()
    }

    /// Logical remote directory of the battery unit
    /// (`/linkSlot/<platform>/<sha>/sav`).
    pub fn remote_sav_unit_dir(&self, unit: &Unit) -> String {
        let mut path = self.remote_root();
        for segment in UnitSpec::sav(unit, &self.root)
            .remote_segments(unit)
            .expect("harness unit carries a ROM hash")
        {
            path.push('/');
            path.push_str(&segment);
        }
        path
    }

    pub fn remote_sav_path(&self, unit: &Unit) -> String {
        format!("{}/battery.sav", self.remote_sav_unit_dir(unit))
    }

    pub fn remote_sav_meta_path(&self, unit: &Unit) -> String {
        format!("{}/meta.json", self.remote_sav_unit_dir(unit))
    }

    /// Seed a cloud battery unit (data file + schema 2 sav meta + layout).
    pub fn seed_remote_sav(&self, unit: &Unit, bytes: &[u8], version: u64, device: &str) -> Meta {
        let battery = (
            "battery.sav".to_string(),
            bytes.len() as u64,
            hash::sha256_hex(bytes),
        );
        let meta = Meta::build_sav(unit, version, device, &battery, None);
        self.dav.insert_file(&self.remote_sav_path(unit), bytes);
        self.dav
            .insert_file(&self.remote_sav_meta_path(unit), &meta.to_bytes().unwrap());
        self.seed_layout();
        meta
    }

    pub fn remote_sav(&self, unit: &Unit) -> Option<Vec<u8>> {
        self.dav.file(&self.remote_sav_path(unit))
    }

    pub fn remote_sav_meta(&self, unit: &Unit) -> Option<Meta> {
        self.dav
            .file(&self.remote_sav_meta_path(unit))
            .map(|bytes| Meta::parse(&bytes).unwrap())
    }

    /// `push_sav` against this harness.
    pub fn push_sav(&self, options: PushOptions) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::push_sav(&self.ctx, &self.config, &unit, options, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    /// `pull_sav` against this harness.
    pub fn pull_sav(&self, options: PullOptions) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::pull_sav(&self.ctx, &self.config, &unit, options, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    /// `<game>.conflict.<stamp>.sav` files next to the local battery save.
    pub fn sav_conflict_files(&self, unit: &Unit) -> Vec<PathBuf> {
        let dir = match self.sav_path(unit).parent() {
            Some(dir) => dir.to_path_buf(),
            None => return Vec::new(),
        };
        let prefix = format!("{}.conflict.", unit.game);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut out: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_type()
                    .map(|kind| kind.is_file())
                    .unwrap_or(false)
            })
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect();
        out.sort();
        out
    }

    /// Battery cache entry of this unit.
    pub fn load_sav_cache(&self, unit: &Unit) -> cache::CacheLoad {
        let spec = UnitSpec::sav(unit, &self.root);
        cache::load(&self.root, &spec, unit, &self.remote_ref()).unwrap()
    }

    // ---- playtime.json ---------------------------------------------------

    /// Local `<unit>/playtime.json` path (never part of the save whitelist).
    pub fn local_playtime_path(&self, unit: &Unit) -> PathBuf {
        self.unit_dir(unit).join(playtime::PLAYTIME)
    }

    /// Write raw local playtime JSON bytes, creating the unit directory.
    pub fn write_local_playtime(&self, unit: &Unit, bytes: &[u8]) -> PathBuf {
        let path = self.local_playtime_path(unit);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, bytes).unwrap();
        path
    }

    pub fn local_playtime(&self, unit: &Unit) -> Option<Vec<u8>> {
        std::fs::read(self.local_playtime_path(unit)).ok()
    }

    /// Cloud `<platform>/<sha>/playtime.json` path (game level, not a unit).
    pub fn remote_playtime_path(&self, unit: &Unit) -> String {
        format!("{}/{}", self.remote_game_dir(unit), playtime::PLAYTIME)
    }

    pub fn seed_remote_playtime(&self, unit: &Unit, bytes: &[u8]) {
        self.dav
            .insert_file(&self.remote_playtime_path(unit), bytes);
    }

    pub fn remote_playtime(&self, unit: &Unit) -> Option<Vec<u8>> {
        self.dav.file(&self.remote_playtime_path(unit))
    }

    /// Cloud `<unit>/.conflict.<stamp>/` backups (sorted), created by
    /// `push --force` / `sync --resolve upload`.
    pub fn remote_conflict_dirs(&self, unit: &Unit) -> Vec<String> {
        let prefix = format!("{}/.conflict.", self.remote_unit_dir(unit));
        self.dav
            .dir_names_under(&prefix)
            .into_iter()
            .filter(|path| {
                path.rsplit('/')
                    .next()
                    .map(|name| name.starts_with(".conflict."))
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Logical remote file paths below `dir` whose direct parent is `dir`.
    pub fn remote_files_in_dir(&self, dir: &str) -> Vec<String> {
        let prefix = format!("{dir}/");
        self.dav
            .file_names_under(&prefix)
            .into_iter()
            .filter(|path| path[prefix.len()..].find('/').is_none())
            .collect()
    }

    pub fn seed_local(&self, unit: &Unit, files: &[(&str, &[u8])]) {
        let dir = self.unit_dir(unit);
        std::fs::create_dir_all(&dir).unwrap();
        for (name, bytes) in files {
            std::fs::write(dir.join(name), bytes).unwrap();
        }
    }

    pub fn local_file(&self, unit: &Unit, name: &str) -> Option<Vec<u8>> {
        std::fs::read(self.unit_dir(unit).join(name)).ok()
    }

    pub fn remove_local(&self, unit: &Unit, name: &str) {
        let path = self.unit_dir(unit).join(name);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }

    /// Build a meta for `files` at `version` and store both files and meta in
    /// the mock server.
    pub fn seed_remote(
        &self,
        unit: &Unit,
        files: &[(&str, &[u8])],
        version: u64,
        device: &str,
    ) -> Meta {
        let hashed: Vec<(String, u64, String)> = files
            .iter()
            .map(|(name, bytes)| {
                (
                    name.to_string(),
                    bytes.len() as u64,
                    hash::sha256_hex(bytes),
                )
            })
            .collect();
        let ring_count = files
            .iter()
            .filter(|(name, _)| linkslot::unit::state_stamp(name).is_some())
            .count();
        let meta = Meta::build(unit, version, device, &hashed, ring_count, None);
        for (name, bytes) in files {
            self.dav.insert_file(&self.remote_path(unit, name), bytes);
        }
        self.dav
            .insert_file(&self.remote_meta_path(unit), &meta.to_bytes().unwrap());
        self.seed_layout();
        meta
    }

    /// Insert an arbitrary meta (e.g. one with a deliberately wrong digest).
    pub fn seed_remote_meta(&self, unit: &Unit, meta: &Meta, files: &[(&str, &[u8])]) {
        for (name, bytes) in files {
            self.dav.insert_file(&self.remote_path(unit, name), bytes);
        }
        self.dav
            .insert_file(&self.remote_meta_path(unit), &meta.to_bytes().unwrap());
        self.seed_layout();
    }

    /// Write the v0.0.10 layout marker at the remote root (what a cloud
    /// written by this build always has).
    pub fn seed_layout(&self) {
        let path = format!("{}/{}", self.remote_root(), LAYOUT_NAME);
        self.dav
            .insert_file(&path, &Layout::current().to_bytes().unwrap());
    }

    pub fn remote_layout_path(&self) -> String {
        format!("{}/{}", self.remote_root(), LAYOUT_NAME)
    }

    /// Overwrite the layout marker with arbitrary bytes (mismatch tests).
    pub fn seed_layout_bytes(&self, bytes: &[u8]) {
        self.dav.insert_file(&self.remote_layout_path(), bytes);
    }

    /// Remove one logical file from the mock cloud.
    pub fn remove_remote_file(&self, path: &str) {
        self.dav.state().lock().unwrap().files.remove(path);
    }

    pub fn seed_cache(&self, unit: &Unit, meta: &Meta) -> Cache {
        let cache = Cache::from_meta(meta, unit, self.remote_ref());
        let spec = UnitSpec::state(unit, &self.root);
        cache::save(&self.root, &spec, unit, &cache).unwrap();
        cache
    }

    pub fn load_cache(&self, unit: &Unit) -> cache::CacheLoad {
        let spec = UnitSpec::state(unit, &self.root);
        cache::load(&self.root, &spec, unit, &self.remote_ref()).unwrap()
    }

    /// Build a meta from the current local whitelist (used to seed caches).
    pub fn meta_from_local(&self, unit: &Unit, version: u64) -> Meta {
        let scan = linkslot::unit::scan(&self.root, unit).unwrap();
        let hashed = linkslot::unit::hash_scan(&scan).unwrap();
        Meta::build(
            unit,
            version,
            "rg34xxsp-deadbeef",
            &hashed,
            scan.ring_count,
            None,
        )
    }

    /// Names of `<game>.conflict.<stamp>` directories next to the unit dir.
    pub fn conflict_dirs(&self, unit: &Unit) -> Vec<PathBuf> {
        let parent = unit.local_parent(&self.root);
        let Ok(entries) = std::fs::read_dir(&parent) else {
            return Vec::new();
        };
        let prefix = format!("{}.conflict.", unit.game);
        let mut out: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect();
        out.sort();
        out
    }

    /// Whitelist files currently present in the local unit directory.
    pub fn local_names(&self, unit: &Unit) -> Vec<String> {
        let dir = self.unit_dir(unit);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().map(|t| t.is_file()).unwrap_or(false))
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        names.sort();
        names
    }

    fn remote_ref(&self) -> RemoteRef {
        RemoteRef {
            url: self.config.webdav().url.clone(),
            remote_root: self.config.webdav().remote_root.clone(),
        }
    }

    pub fn push(&self, options: PushOptions) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::push(&self.ctx, &self.config, &unit, options, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    pub fn pull(&self, options: PullOptions) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::pull(&self.ctx, &self.config, &unit, options, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    pub fn status(&self) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::status(&self.ctx, &self.config, &unit, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    pub fn sync(&self, options: SyncOptions) -> RunResult {
        let unit = self.unit();
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let outcome = {
            let mut reporter = Reporter::new(&self.root, true, &mut stdout, &mut stderr);
            let outcome = sync::sync(&self.ctx, &self.config, &unit, options, &mut reporter);
            reporter.finish(&outcome);
            outcome
        };
        RunResult {
            outcome,
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        }
    }

    /// Run the real CLI entry point against this harness.
    pub fn cli(&self, args: &[&str]) -> (i32, String, String) {
        let mut full: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        full.push("--root".to_string());
        full.push(self.root.to_string_lossy().to_string());
        full.push("--config".to_string());
        full.push(self.config.path.to_string_lossy().to_string());
        let mut stdout: Vec<u8> = Vec::new();
        let mut stderr: Vec<u8> = Vec::new();
        let code = linkslot::cli::execute(&full, &mut stdout, &mut stderr);
        (
            code,
            String::from_utf8_lossy(&stdout).to_string(),
            String::from_utf8_lossy(&stderr).to_string(),
        )
    }

    pub fn config_dir(&self) -> PathBuf {
        self.root.join("linkSlot/Config")
    }
}

/// Parse the final `LINKSLOT_*` block into ordered (key, value) pairs.
pub fn parse_contract(stdout: &str) -> Vec<(String, String)> {
    stdout
        .lines()
        .filter(|line| line.starts_with("LINKSLOT_") && !line.starts_with("LINKSLOT_STAGE="))
        .filter_map(|line| {
            line.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
}

pub fn contract_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}

/// The final `LINKSLOT_*` block only: skips stage lines *and* the early
/// `LINKSLOT_DIRECTION=` line that `sync` emits before transferring.
pub fn final_contract(stdout: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = stdout.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.starts_with("LINKSLOT_ACTION="))
        .unwrap_or(0);
    parse_contract(&lines[start..].join("\n"))
}

/// Every occurrence of `key` in stdout (stage lines excluded), in order.
pub fn contract_values(stdout: &str, key: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| !line.starts_with("LINKSLOT_STAGE="))
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| *name == key)
        .map(|(_, value)| value.to_string())
        .collect()
}

pub fn stage_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter_map(|line| line.strip_prefix("LINKSLOT_STAGE="))
        .map(str::to_string)
        .collect()
}
