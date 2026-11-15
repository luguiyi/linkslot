//! Minimal synchronous WebDAV client.
//!
//! Why not `ureq` directly? ureq 3.4's `Agent::run()` verifies the HTTP method
//! against its fixed standard-method list and rejects WebDAV methods (`MKCOL`,
//! `MOVE`, …) with `MethodVersionMismatch` before any bytes are sent. linkSlot
//! needs those methods, so this module speaks HTTP/1.1 itself over
//! `std::net::TcpStream` (plain HTTP) or `rustls` (HTTPS, `tls-rustls` feature).
//! The `ureq` dependency/feature stays declared because the task's dependency
//! contract enables the rustls/ring stack through it, and so a future ureq
//! release with custom-method support can replace this transport.
//!
//! Properties kept from the plan:
//! - Basic auth header only (never logged);
//! - per-segment percent-encoding for Chinese game names;
//! - GET 404 -> `Ok(None)`;
//! - retries only for retryable errors (connect/timeout/5xx);
//! - TLS: built-in webpki roots by default, optional `ca_cert` PEM anchors,
//!   `insecure` for debugging; cert/clock errors are classified precisely from
//!   rustls' `CertificateError`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

#[cfg(feature = "mods")]
use percent_encoding::percent_decode_str;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::config::{split_remote_root, Config};
use crate::error::{ErrorKind, LinkError, Result};
use crate::output::Reporter;

const ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

const RETRY_BASE_MS: u64 = 250;
const USER_AGENT: &str = concat!("linkSlot/", env!("CARGO_PKG_VERSION"));

/// Upper bound for a PROPFIND response body (Lua mods remote listing).
#[cfg(feature = "mods")]
const MAX_PROPFIND_BODY: usize = 2 * 1024 * 1024;
/// Upper bound for the number of entries returned by [`DavClient::list`].
#[cfg(feature = "mods")]
const MAX_LIST_ENTRIES: usize = 1000;

/// Parsed WebDAV endpoint. Userinfo in the URL is rejected so credentials
/// cannot end up in logs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    /// Base path as configured (`/dav`), already percent-encoded by the user.
    pub base_path: String,
    /// `host[:port]` form used to rebuild URLs (IPv6 in brackets).
    authority: String,
}

impl Endpoint {
    pub fn parse(url: &str) -> Result<Endpoint> {
        let url = url.trim();
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| LinkError::config(format!("缺少 scheme：{url:?}")))?;
        let tls = match scheme.to_ascii_lowercase().as_str() {
            "https" => true,
            "http" => false,
            other => {
                return Err(LinkError::config(format!(
                    "不支持的 scheme {other:?}（仅支持 http/https）"
                )));
            }
        };
        if rest.contains('?') || rest.contains('#') {
            return Err(LinkError::config("不能包含 query 或 fragment"));
        }
        if rest.is_empty() {
            return Err(LinkError::config("缺少主机名"));
        }
        let (authority_raw, base_path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };
        if authority_raw.contains('@') {
            return Err(LinkError::config(
                "URL 中不能包含用户名密码，请使用 webdav.username/password",
            ));
        }
        if authority_raw.is_empty() {
            return Err(LinkError::config("缺少主机名"));
        }

        let (host, port_text) = if let Some(rest) = authority_raw.strip_prefix('[') {
            let end = rest
                .find(']')
                .ok_or_else(|| LinkError::config("IPv6 地址缺少 ]"))?;
            let host = &rest[..end];
            let tail = &rest[end + 1..];
            let port = match tail.strip_prefix(':') {
                Some(port) => Some(port),
                None if tail.is_empty() => None,
                None => return Err(LinkError::config("IPv6 地址格式无效")),
            };
            (host.to_string(), port.map(str::to_string))
        } else {
            match authority_raw.rfind(':') {
                Some(index)
                    if authority_raw[index + 1..]
                        .chars()
                        .all(|c| c.is_ascii_digit()) =>
                {
                    (
                        authority_raw[..index].to_string(),
                        Some(authority_raw[index + 1..].to_string()),
                    )
                }
                _ if authority_raw.contains(':') => {
                    return Err(LinkError::config("主机名或端口无效"));
                }
                _ => (authority_raw.to_string(), None),
            }
        };
        if host.is_empty() || host.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(LinkError::config("主机名无效"));
        }
        let explicit_port = port_text.is_some();
        let port = match port_text {
            Some(text) => text
                .parse::<u16>()
                .map_err(|_| LinkError::config(format!("端口无效：{text:?}")))?,
            None => {
                if tls {
                    443
                } else {
                    80
                }
            }
        };
        if port == 0 {
            return Err(LinkError::config("端口无效：0"));
        }
        let base_path = normalize_base_path(base_path);
        let authority = if host.contains(':') {
            if explicit_port {
                format!("[{host}]:{port}")
            } else {
                format!("[{host}]")
            }
        } else if explicit_port {
            format!("{host}:{port}")
        } else {
            host.clone()
        };
        Ok(Endpoint {
            tls,
            host,
            port,
            base_path,
            authority,
        })
    }

    pub fn scheme(&self) -> &'static str {
        if self.tls {
            "https"
        } else {
            "http"
        }
    }

    pub fn base_url(&self) -> String {
        format!("{}://{}{}", self.scheme(), self.authority, self.base_path)
    }

    /// Host name suitable for DNS resolution / socket connect.
    pub fn socket_host(&self) -> &str {
        &self.host
    }
}

fn normalize_base_path(path: &str) -> String {
    let path = path.trim_end_matches('/');
    if path.is_empty() {
        String::new()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Percent-encode one path segment, keeping only unreserved characters.
pub fn encode_segment(segment: &str) -> String {
    utf8_percent_encode(segment, ENCODE_SET).to_string()
}

/// Percent-encode each segment and join with `/` (no leading slash).
pub fn encode_segments<'a, I>(segments: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    segments
        .into_iter()
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// Percent-encode one query value (keeps unreserved characters).
#[cfg(feature = "mods")]
pub fn encode_query_value(value: &str) -> String {
    utf8_percent_encode(value, ENCODE_SET).to_string()
}

/// Base64 (standard alphabet, with padding) for the Basic auth header.
pub fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[((n >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// One entry of a WebDAV collection, as returned by [`DavClient::list`] and
/// [`DavClient::stat`].
///
/// `name` is the decoded last href segment (so Chinese names are usable as path
/// segments again); `modified` keeps the server's `getlastmodified` string
/// untouched instead of reformatting it.
#[cfg(feature = "mods")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DavEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveOutcome {
    Moved,
    /// Server does not implement MOVE (405/501); caller falls back to PUT.
    Unsupported,
}

trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// Connection parameters for one WebDAV target.
///
/// The main sync target and the scoped mod/HTTP transports share this
/// transport but never share configuration.
pub struct DavTarget<'a> {
    pub endpoint: &'a Endpoint,
    /// Already normalized remote root (`/linkSlot` or empty).
    pub remote_root: &'a str,
    pub username: &'a str,
    pub password: &'a str,
    pub ca_cert: &'a str,
    pub insecure: bool,
    pub retries: u32,
    pub timeout_secs: u64,
    /// Prefix for human-readable messages, e.g. `webdav` or `mod`.
    pub label: &'a str,
}

/// Synchronous WebDAV client scoped to one configured remote root.
pub struct DavClient {
    endpoint: Endpoint,
    remote_segments: Vec<String>,
    auth: Option<String>,
    retries: u32,
    timeout: Duration,
    #[cfg(feature = "tls-rustls")]
    tls: Option<std::sync::Arc<rustls::ClientConfig>>,
}

impl DavClient {
    pub fn new(config: &Config, rep: &mut Reporter) -> Result<DavClient> {
        let target = DavTarget {
            endpoint: config.endpoint(),
            remote_root: &config.webdav().remote_root,
            username: &config.webdav().username,
            password: &config.webdav().password,
            ca_cert: &config.webdav().ca_cert,
            insecure: config.webdav().insecure,
            retries: config.sync_cfg().retries,
            timeout_secs: config.sync_cfg().timeout_secs,
            label: "webdav",
        };
        DavClient::from_target(&target, rep)
    }

    /// Build a client for an arbitrary target (for example a mod's HTTP
    /// origin).  The caller validates the endpoint/root.
    pub fn from_target(target: &DavTarget<'_>, rep: &mut Reporter) -> Result<DavClient> {
        let endpoint = target.endpoint.clone();
        let auth = if target.username.is_empty() {
            None
        } else {
            let raw = format!("{}:{}", target.username, target.password);
            Some(format!("Basic {}", base64_encode(raw.as_bytes())))
        };
        if !endpoint.tls {
            rep.info(format!("{} 使用明文 HTTP（仅调试）", target.label));
        }
        #[cfg(feature = "tls-rustls")]
        let tls = build_tls_config(
            &endpoint,
            target.ca_cert,
            target.insecure,
            target.label,
            rep,
        )?;
        #[cfg(not(feature = "tls-rustls"))]
        if endpoint.tls {
            return Err(LinkError::config(
                "本构建不支持 HTTPS（未启用 tls-rustls feature）",
            ));
        }
        Ok(DavClient {
            endpoint,
            remote_segments: split_remote_root(target.remote_root),
            auth,
            retries: target.retries,
            timeout: Duration::from_secs(target.timeout_secs),
            #[cfg(feature = "tls-rustls")]
            tls,
        })
    }

    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Check that the configured WebDAV root is reachable and accepts the
    /// account.  PROPFIND on the remote root verifies DNS/TCP/TLS, credentials
    /// and the remote collection in one request; 207 is the normal WebDAV
    /// multi-status reply.
    pub fn probe(&self, rep: &mut Reporter) -> Result<u16> {
        let url = self.url_for(&[]);
        let response = self.send(
            rep,
            "PROPFIND",
            &url,
            &[("Depth", "0".to_string())],
            None,
            None,
        )?;
        match response.status {
            200..=299 => Ok(response.status),
            404 | 410 => Err(LinkError::new(
                ErrorKind::Server,
                "远程根目录不存在（HTTP 404），请检查 webdav.remote_root",
            )),
            405 => Err(LinkError::new(
                ErrorKind::Server,
                "服务器不接受 PROPFIND（HTTP 405），可能不是 WebDAV 端点",
            )),
            status => Err(status_error(status)),
        }
    }

    /// Absolute URL for a path relative to the configured remote root.
    pub fn url_for(&self, rel: &[&str]) -> String {
        let mut url = self.endpoint.base_url();
        for segment in &self.remote_segments {
            url.push('/');
            url.push_str(&encode_segment(segment));
        }
        for segment in rel {
            url.push('/');
            url.push_str(&encode_segment(segment));
        }
        url
    }

    /// Perform one request with retries.
    ///
    /// `max_body` optionally bounds the response body (header + body guard);
    /// `None` keeps the historical unbounded behavior.  A body-limit violation
    /// is not retried.
    fn send(
        &self,
        rep: &mut Reporter,
        method: &str,
        url: &str,
        extra: &[(&str, String)],
        body: Option<&[u8]>,
        max_body: Option<usize>,
    ) -> Result<DavResponse> {
        let mut last_error: Option<LinkError> = None;
        for attempt in 0..=self.retries {
            if attempt > 0 {
                let delay = RETRY_BASE_MS * (1u64 << (attempt - 1).min(4));
                rep.info(format!(
                    "{method} 重试 {attempt}/{}（{delay}ms 后）",
                    self.retries
                ));
                std::thread::sleep(Duration::from_millis(delay));
            }
            let mut stream = match self.connect() {
                Ok(stream) => stream,
                Err(error) => {
                    if error.kind().is_retryable() && attempt < self.retries {
                        rep.info(format!("{method} 连接失败，将重试：{}", error.message()));
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            };
            match self.exchange(&mut stream, method, url, extra, body, max_body) {
                Ok(response) => {
                    if response.status >= 500 && attempt < self.retries {
                        last_error = Some(status_error(response.status));
                        continue;
                    }
                    return Ok(response);
                }
                Err(ReadError::TooLarge(limit)) => return Err(body_too_large(limit)),
                Err(ReadError::Link(error)) => {
                    if error.kind().is_retryable() && attempt < self.retries {
                        rep.info(format!("{method} 失败，将重试：{}", error.message()));
                        last_error = Some(error);
                        continue;
                    }
                    return Err(error);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| LinkError::new(ErrorKind::Net, "请求失败")))
    }

    /// One request without retries or a [`Reporter`]; used by the manual mods
    /// HTTP bridge (`http.*`).  Reuses the same transport, TLS setup, timeouts
    /// and response-body guards as the WebDAV client.
    #[cfg(feature = "mods")]
    pub fn request_once(
        &self,
        method: &str,
        url: &str,
        headers: &[(&str, String)],
        body: Option<&[u8]>,
        max_body: Option<usize>,
    ) -> Result<DavResponse> {
        let mut stream = self.connect()?;
        self.exchange(&mut stream, method, url, headers, body, max_body)
            .map_err(|error| match error {
                ReadError::TooLarge(limit) => body_too_large(limit),
                ReadError::Link(error) => error,
            })
    }

    fn connect(&self) -> Result<Box<dyn ReadWrite>> {
        let addrs = (self.endpoint.host.as_str(), self.endpoint.port)
            .to_socket_addrs()
            .map_err(|err| {
                LinkError::new(
                    ErrorKind::Net,
                    format!("无法解析主机名 {}：{err}", self.endpoint.host),
                )
            })?;
        let mut last_error: Option<std::io::Error> = None;
        let mut stream: Option<TcpStream> = None;
        for addr in addrs {
            match TcpStream::connect_timeout(&addr, self.timeout) {
                Ok(connected) => {
                    stream = Some(connected);
                    break;
                }
                Err(err) => last_error = Some(err),
            }
        }
        let stream = stream.ok_or_else(|| {
            let detail = last_error
                .map(|err| err.to_string())
                .unwrap_or_else(|| "没有可用地址".to_string());
            LinkError::new(
                ErrorKind::Net,
                format!(
                    "无法连接 {}:{}（{detail}）",
                    self.endpoint.host, self.endpoint.port
                ),
            )
        })?;
        stream.set_read_timeout(Some(self.timeout)).ok();
        stream.set_write_timeout(Some(self.timeout)).ok();
        if !self.endpoint.tls {
            return Ok(Box::new(stream));
        }
        #[cfg(feature = "tls-rustls")]
        {
            let config = self
                .tls
                .as_ref()
                .ok_or_else(|| LinkError::config("HTTPS 配置缺失（内部错误）"))?;
            let server_name = server_name(&self.endpoint.host)?;
            let conn = rustls::ClientConnection::new(std::sync::Arc::clone(config), server_name)
                .map_err(|err| LinkError::new(ErrorKind::Cert, format!("TLS 初始化失败：{err}")))?;
            let mut tls = rustls::StreamOwned { conn, sock: stream };
            while tls.conn.is_handshaking() {
                if let Err(err) = tls.conn.complete_io(&mut tls.sock) {
                    return Err(classify_io_error(err));
                }
            }
            Ok(Box::new(tls))
        }
        #[cfg(not(feature = "tls-rustls"))]
        Err(LinkError::config(
            "本构建不支持 HTTPS（未启用 tls-rustls feature）",
        ))
    }

    fn exchange(
        &self,
        stream: &mut Box<dyn ReadWrite>,
        method: &str,
        url: &str,
        extra: &[(&str, String)],
        body: Option<&[u8]>,
        max_body: Option<usize>,
    ) -> std::result::Result<DavResponse, ReadError> {
        let target = request_target(url)?;
        let mut head = format!(
            "{method} {target} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {USER_AGENT}\r\nConnection: close\r\n",
            self.endpoint.authority
        );
        if let Some(auth) = &self.auth {
            head.push_str(&format!("Authorization: {auth}\r\n"));
        }
        for (name, value) in extra {
            if value.contains('\r') || value.contains('\n') {
                return Err(LinkError::config(format!("非法请求头 {name}")).into());
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        if let Some(bytes) = body {
            head.push_str(&format!("Content-Length: {}\r\n", bytes.len()));
        }
        head.push_str("\r\n");
        stream
            .write_all(head.as_bytes())
            .map_err(classify_io_error)?;
        if let Some(bytes) = body {
            stream.write_all(bytes).map_err(classify_io_error)?;
        }
        stream.flush().map_err(classify_io_error)?;
        read_response(stream, max_body)
    }

    /// GET; `Ok(None)` on HTTP 404 (missing meta or file).
    pub fn get(&self, rep: &mut Reporter, rel: &[&str]) -> Result<Option<Vec<u8>>> {
        let url = self.url_for(rel);
        let response = self.send(rep, "GET", &url, &[], None, None)?;
        match response.status {
            200..=299 => Ok(Some(response.body)),
            404 | 410 => Ok(None),
            status => Err(status_error(status)),
        }
    }

    /// GET with a hard response-body limit; `Ok(None)` on HTTP 404.
    ///
    /// The limit is enforced while reading (not only on `Content-Length`), so a
    /// huge or chunked remote file cannot exhaust memory.  Used by the Lua mods
    /// runtime (`webdav.get`, 16 MiB).
    pub fn get_limited(
        &self,
        rep: &mut Reporter,
        rel: &[&str],
        max_bytes: usize,
    ) -> Result<Option<Vec<u8>>> {
        let url = self.url_for(rel);
        let response = self.send(rep, "GET", &url, &[], None, Some(max_bytes))?;
        match response.status {
            200..=299 => {
                if response.body.len() > max_bytes {
                    return Err(body_too_large(max_bytes));
                }
                Ok(Some(response.body))
            }
            404 | 410 => Ok(None),
            status => Err(status_error(status)),
        }
    }

    /// List a collection (PROPFIND `Depth: 1`); `Ok(None)` on HTTP 404/410.
    ///
    /// The server's own entry is skipped; the XML body is capped at 2 MiB and
    /// the returned entry count at 1000.  Needs the `mods` feature (the
    /// roxmltree-based parser is only compiled for the Lua runtime).
    #[cfg(feature = "mods")]
    pub fn list(&self, rep: &mut Reporter, rel: &[&str]) -> Result<Option<Vec<DavEntry>>> {
        let url = self.url_for(rel);
        let response = self.send(
            rep,
            "PROPFIND",
            &url,
            &[("Depth", "1".to_string())],
            None,
            Some(MAX_PROPFIND_BODY),
        )?;
        match response.status {
            200..=299 => {
                let xml = propfind_xml(&response.body)?;
                let self_segments = request_segments(&url)?;
                list_entries_from_multistatus(xml, &self_segments).map(Some)
            }
            404 | 410 => Ok(None),
            status => Err(status_error(status)),
        }
    }

    /// Stat one resource (PROPFIND `Depth: 0`); `Ok(None)` on HTTP 404/410.
    ///
    /// Needs the `mods` feature (roxmltree).
    #[cfg(feature = "mods")]
    pub fn stat(&self, rep: &mut Reporter, rel: &[&str]) -> Result<Option<DavEntry>> {
        let url = self.url_for(rel);
        let response = self.send(
            rep,
            "PROPFIND",
            &url,
            &[("Depth", "0".to_string())],
            None,
            Some(MAX_PROPFIND_BODY),
        )?;
        match response.status {
            200..=299 => {
                let xml = propfind_xml(&response.body)?;
                let self_segments = request_segments(&url)?;
                stat_entry_from_multistatus(xml, &self_segments)
            }
            404 | 410 => Ok(None),
            status => Err(status_error(status)),
        }
    }

    /// PUT (overwrites, creates parent-independent file).
    pub fn put(&self, rep: &mut Reporter, rel: &[&str], bytes: &[u8]) -> Result<()> {
        let url = self.url_for(rel);
        let headers = [("Content-Type", "application/octet-stream".to_string())];
        let response = self.send(rep, "PUT", &url, &headers, Some(bytes), None)?;
        match response.status {
            200..=299 => Ok(()),
            409 => Err(LinkError::new(
                ErrorKind::Server,
                "PUT 失败：云端目录不存在（HTTP 409）",
            )),
            status => Err(status_error(status)),
        }
    }

    /// DELETE; `Ok(false)` when the resource did not exist.
    pub fn delete(&self, rep: &mut Reporter, rel: &[&str]) -> Result<bool> {
        let url = self.url_for(rel);
        let response = self.send(rep, "DELETE", &url, &[], None, None)?;
        match response.status {
            200..=299 => Ok(true),
            404 | 410 => Ok(false),
            status => Err(status_error(status)),
        }
    }

    /// MKCOL; an existing collection (HTTP 405) counts as success.
    pub fn mkcol(&self, rep: &mut Reporter, rel: &[&str]) -> Result<()> {
        let url = self.url_for(rel);
        let response = self.send(rep, "MKCOL", &url, &[], None, None)?;
        match response.status {
            200..=299 => Ok(()),
            405 => Ok(()),
            409 => Err(LinkError::new(
                ErrorKind::Server,
                "MKCOL 失败：父目录不存在（HTTP 409）",
            )),
            status => Err(status_error(status)),
        }
    }

    /// Create the remote root and every directory up to `rel`.
    pub fn ensure_dir(&self, rep: &mut Reporter, rel: &[&str]) -> Result<()> {
        for depth in 0..=rel.len() {
            self.mkcol(rep, &rel[..depth])?;
        }
        Ok(())
    }

    /// MOVE with `Overwrite: T`; `Unsupported` when the server rejects MOVE
    /// with 405/501 so the caller can fall back to a direct PUT.
    pub fn move_to(&self, rep: &mut Reporter, from: &[&str], to: &[&str]) -> Result<MoveOutcome> {
        let from_url = self.url_for(from);
        let to_url = self.url_for(to);
        let headers = [
            ("Destination", to_url.clone()),
            ("Overwrite", "T".to_string()),
        ];
        let response = self.send(rep, "MOVE", &from_url, &headers, None, None)?;
        match response.status {
            200..=299 => Ok(MoveOutcome::Moved),
            405 | 501 => Ok(MoveOutcome::Unsupported),
            404 | 410 => Err(LinkError::new(
                ErrorKind::Server,
                "MOVE 失败：源文件不存在（HTTP 404）",
            )),
            409 => Err(LinkError::new(
                ErrorKind::Server,
                "MOVE 失败：目标目录不存在（HTTP 409）",
            )),
            412 => Err(LinkError::new(
                ErrorKind::Server,
                "MOVE 失败：目标已存在且服务器拒绝覆盖（HTTP 412）",
            )),
            status => Err(status_error(status)),
        }
    }
}

// ---------------------------------------------------------------------------
// PROPFIND multistatus parsing (`mods` feature only; uses roxmltree)
// ---------------------------------------------------------------------------

#[cfg(feature = "mods")]
#[derive(Clone, Debug)]
struct RawDavEntry {
    href: String,
    is_dir: bool,
    size: u64,
    modified: Option<String>,
}

#[cfg(feature = "mods")]
fn propfind_xml(body: &[u8]) -> Result<&str> {
    std::str::from_utf8(body)
        .map_err(|_| LinkError::new(ErrorKind::Server, "PROPFIND 响应不是 UTF-8"))
}

/// Parse a `<multistatus>` document tolerantly: any namespace prefix, absolute
/// URLs or relative `href`s, percent-encoded names and empty collections.
#[cfg(feature = "mods")]
fn parse_multistatus(xml: &str) -> Result<Vec<RawDavEntry>> {
    let xml = xml.trim_start_matches('\u{feff}');
    let document = roxmltree::Document::parse(xml).map_err(|err| {
        LinkError::new(
            ErrorKind::Server,
            format!("PROPFIND 响应 XML 解析失败：{err}"),
        )
    })?;
    let root = document
        .root()
        .first_element_child()
        .ok_or_else(|| LinkError::new(ErrorKind::Server, "PROPFIND 响应缺少 multistatus 根元素"))?;
    if root.tag_name().name() != "multistatus" {
        return Err(LinkError::new(
            ErrorKind::Server,
            format!(
                "PROPFIND 响应根元素不是 multistatus（{}）",
                root.tag_name().name()
            ),
        ));
    }
    let mut entries = Vec::new();
    for response in root
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "response")
    {
        let href = response
            .children()
            .find(|node| node.is_element() && node.tag_name().name() == "href")
            .and_then(|node| node.text())
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .ok_or_else(|| LinkError::new(ErrorKind::Server, "PROPFIND 响应条目缺少 href"))?;
        let mut entry = RawDavEntry {
            href: href.to_string(),
            is_dir: href_path(href).ends_with('/'),
            size: 0,
            modified: None,
        };
        for prop in response
            .descendants()
            .filter(|node| node.is_element() && node.tag_name().name() == "prop")
        {
            // Skip 404 propstat blocks (`<status>HTTP/1.1 404 Not Found</status>`).
            if let Some(propstat) = prop
                .parent()
                .filter(|node| node.is_element() && node.tag_name().name() == "propstat")
            {
                let success = propstat
                    .children()
                    .find(|node| node.is_element() && node.tag_name().name() == "status")
                    .and_then(|node| node.text())
                    .is_none_or(is_success_status);
                if !success {
                    continue;
                }
            }
            for child in prop.children().filter(|node| node.is_element()) {
                match child.tag_name().name() {
                    "resourcetype" => {
                        entry.is_dir |= child.children().any(|node| {
                            node.is_element() && node.tag_name().name() == "collection"
                        });
                    }
                    "getcontentlength" => {
                        entry.size = child
                            .text()
                            .and_then(|text| text.trim().parse::<u64>().ok())
                            .unwrap_or(0);
                    }
                    "getlastmodified" => {
                        let text = child.text().map(str::trim).unwrap_or("");
                        if !text.is_empty() {
                            entry.modified = Some(text.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(feature = "mods")]
fn is_success_status(status: &str) -> bool {
    status
        .split_whitespace()
        .find_map(|part| part.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

#[cfg(feature = "mods")]
fn list_entries_from_multistatus(xml: &str, self_segments: &[String]) -> Result<Vec<DavEntry>> {
    let raw_entries = parse_multistatus(xml)?;
    let mut entries = Vec::new();
    for raw in &raw_entries {
        if is_self_href(&raw.href, self_segments) {
            continue;
        }
        entries.push(dav_entry(raw, self_segments));
        if entries.len() > MAX_LIST_ENTRIES {
            return Err(LinkError::new(
                ErrorKind::Server,
                format!("目录条目超过上限（{MAX_LIST_ENTRIES} 条）"),
            ));
        }
    }
    Ok(entries)
}

#[cfg(feature = "mods")]
fn stat_entry_from_multistatus(xml: &str, self_segments: &[String]) -> Result<Option<DavEntry>> {
    Ok(parse_multistatus(xml)?
        .first()
        .map(|raw| dav_entry(raw, self_segments)))
}

#[cfg(feature = "mods")]
fn dav_entry(raw: &RawDavEntry, self_segments: &[String]) -> DavEntry {
    let name = href_name(&raw.href)
        .or_else(|| self_segments.last().cloned())
        .unwrap_or_default();
    DavEntry {
        name,
        is_dir: raw.is_dir,
        size: if raw.is_dir { 0 } else { raw.size },
        modified: raw.modified.clone(),
    }
}

/// True when an href points at the requested collection itself (servers return
/// it as the first entry of a `Depth: 1` response).
#[cfg(feature = "mods")]
fn is_self_href(href: &str, self_segments: &[String]) -> bool {
    let path = href_path(href);
    if path.is_empty() {
        return false;
    }
    let resolved = if path.starts_with('/') {
        split_decoded_path(path)
    } else {
        let mut resolved = self_segments.to_vec();
        for segment in path.split('/') {
            if segment.is_empty() || segment == "." {
                continue;
            }
            if segment == ".." {
                resolved.pop();
                continue;
            }
            resolved.push(decode_segment(segment));
        }
        resolved
    };
    if resolved == self_segments {
        return true;
    }
    // Some servers omit the endpoint base path from absolute hrefs; treat such
    // a shorter path as the requested collection itself.
    !resolved.is_empty() && self_segments.ends_with(&resolved)
}

/// Extract the path part of an absolute URL / scheme-relative URL, or the href
/// itself, and drop any query/fragment.
#[cfg(feature = "mods")]
fn href_path(href: &str) -> &str {
    let href = href.trim();
    let path = if let Some(index) = href.find("://") {
        let rest = &href[index + 3..];
        match rest.find('/') {
            Some(index) => &rest[index..],
            None => "/",
        }
    } else if let Some(rest) = href.strip_prefix("//") {
        match rest.find('/') {
            Some(index) => &rest[index..],
            None => "/",
        }
    } else {
        href
    };
    match path.find(['?', '#']) {
        Some(index) => &path[..index],
        None => path,
    }
}

#[cfg(feature = "mods")]
fn split_decoded_path(path: &str) -> Vec<String> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(decode_segment)
        .collect()
}

#[cfg(feature = "mods")]
fn decode_segment(segment: &str) -> String {
    percent_decode_str(segment).decode_utf8_lossy().into_owned()
}

#[cfg(feature = "mods")]
fn href_name(href: &str) -> Option<String> {
    split_decoded_path(href_path(href))
        .into_iter()
        .rev()
        .find(|segment| segment != "." && segment != "..")
}

/// Extract the origin-form request target from an absolute URL.
fn request_target(url: &str) -> Result<&str> {
    let rest = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .ok_or_else(|| LinkError::config(format!("URL 无效：{url}")))?;
    match rest.find('/') {
        Some(index) => Ok(&rest[index..]),
        None => Ok("/"),
    }
}

/// Decoded path segments of a request URL, used to recognise the self entry.
#[cfg(feature = "mods")]
fn request_segments(url: &str) -> Result<Vec<String>> {
    Ok(split_decoded_path(request_target(url)?))
}

/// Transport-level response error. `TooLarge` is kept separate so a body-limit
/// violation is reported without being retried by [`DavClient::send`].
#[derive(Debug)]
enum ReadError {
    Link(LinkError),
    TooLarge(usize),
}

impl From<LinkError> for ReadError {
    fn from(error: LinkError) -> Self {
        ReadError::Link(error)
    }
}

fn body_too_large(limit: usize) -> LinkError {
    LinkError::new(
        ErrorKind::Server,
        format!("WebDAV 响应体超过上限（{limit} 字节）"),
    )
}

fn read_response(
    stream: &mut Box<dyn ReadWrite>,
    max_body: Option<usize>,
) -> std::result::Result<DavResponse, ReadError> {
    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    reader
        .read_line(&mut status_line)
        .map_err(classify_io_error)?;
    let mut parts = status_line.split_whitespace();
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(LinkError::new(
            ErrorKind::Server,
            format!("WebDAV 响应无效：{status_line:?}"),
        )
        .into());
    }
    let status: u16 = parts
        .next()
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| LinkError::new(ErrorKind::Server, "WebDAV 响应缺少状态码"))?;

    let mut headers: Vec<(String, String)> = Vec::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).map_err(classify_io_error)?;
        if read == 0 {
            break;
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let header = |name: &str| -> Option<&str> {
        headers
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };

    if matches!(status, 204 | 304) {
        return Ok(DavResponse {
            status,
            body: Vec::new(),
        });
    }
    let success = (200..300).contains(&status);
    if max_body.is_some() && !success {
        // Error/redirect bodies are never used by the limited callers; do not
        // let a large error page trip the body cap (a 404 still maps to None).
        return Ok(DavResponse {
            status,
            body: Vec::new(),
        });
    }
    let limit = if success { max_body } else { None };
    let chunked = header("transfer-encoding")
        .map(|value| value.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);
    let body = if chunked {
        read_chunked(&mut reader, limit)?
    } else if let Some(length) = header("content-length") {
        let length: usize = length.parse().map_err(|_| {
            LinkError::new(
                ErrorKind::Server,
                format!("WebDAV 响应的 Content-Length 无效：{length:?}"),
            )
        })?;
        if let Some(max) = limit {
            if length > max {
                return Err(ReadError::TooLarge(max));
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).map_err(classify_io_error)?;
        body
    } else {
        let mut body = Vec::new();
        match limit {
            Some(max) => {
                reader
                    .take((max as u64).saturating_add(1))
                    .read_to_end(&mut body)
                    .map_err(classify_io_error)?;
                if body.len() > max {
                    return Err(ReadError::TooLarge(max));
                }
            }
            None => {
                reader.read_to_end(&mut body).map_err(classify_io_error)?;
            }
        }
        body
    };
    Ok(DavResponse { status, body })
}

fn read_chunked(
    reader: &mut BufReader<&mut Box<dyn ReadWrite>>,
    max_body: Option<usize>,
) -> std::result::Result<Vec<u8>, ReadError> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(classify_io_error)?;
        let size_text = line.trim().split(';').next().unwrap_or("").trim();
        let size = u64::from_str_radix(size_text, 16).map_err(|_| {
            LinkError::new(
                ErrorKind::Server,
                format!("WebDAV chunked 响应块大小无效：{size_text:?}"),
            )
        })?;
        if size == 0 {
            // Trailer headers, terminated by an empty line.
            loop {
                let mut trailer = String::new();
                let read = reader.read_line(&mut trailer).map_err(classify_io_error)?;
                if read == 0 || trailer.trim_end_matches(['\r', '\n']).is_empty() {
                    break;
                }
            }
            break;
        }
        let size = usize::try_from(size)
            .map_err(|_| LinkError::new(ErrorKind::Server, "WebDAV chunked 响应块过大"))?;
        if let Some(max) = max_body {
            if body.len().saturating_add(size) > max {
                return Err(ReadError::TooLarge(max));
            }
        }
        let mut chunk = vec![0u8; size];
        reader.read_exact(&mut chunk).map_err(classify_io_error)?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(classify_io_error)?;
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn status_error(status: u16) -> LinkError {
    match status {
        401 | 403 | 407 => LinkError::new(
            ErrorKind::Auth,
            format!("认证失败（HTTP {status}），请检查 webdav.username/password"),
        ),
        408 => LinkError::new(ErrorKind::Timeout, "服务器请求超时（HTTP 408）"),
        423 => LinkError::new(ErrorKind::Server, "云端资源被锁定（HTTP 423）"),
        429 => LinkError::new(ErrorKind::Server, "云端请求过多（HTTP 429）"),
        300..=399 => LinkError::new(
            ErrorKind::Server,
            format!("未预期的重定向（HTTP {status}），请检查 webdav.url"),
        ),
        500..=599 => LinkError::new(ErrorKind::Server, format!("服务器错误（HTTP {status}）")),
        _ => LinkError::new(
            ErrorKind::Server,
            format!("WebDAV 请求失败（HTTP {status}）"),
        ),
    }
}

fn classify_io_error(err: std::io::Error) -> LinkError {
    if err.kind() == std::io::ErrorKind::TimedOut || err.kind() == std::io::ErrorKind::WouldBlock {
        return LinkError::new(ErrorKind::Timeout, "网络请求超时");
    }
    #[cfg(feature = "tls-rustls")]
    if let Some(tls_error) = err
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        return classify_rustls_error(tls_error);
    }
    if matches!(
        err.kind(),
        std::io::ErrorKind::ConnectionRefused
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::NetworkUnreachable
            | std::io::ErrorKind::HostUnreachable
            | std::io::ErrorKind::UnexpectedEof
    ) {
        return LinkError::new(ErrorKind::Net, format!("网络错误：{err}"));
    }
    classify_tls(&err.to_string())
}

/// Map a TLS error string onto the cert/clock/net categories. Used when the
/// underlying error cannot be downcast to `rustls::Error` (e.g. alert text).
pub fn classify_tls(message: &str) -> LinkError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("expired") || lower.contains("notvalidyet") || lower.contains("not valid yet")
    {
        return LinkError::new(ErrorKind::Clock, "证书有效期校验失败，设备时钟可能不正确");
    }
    if lower.contains("unknownissuer")
        || lower.contains("unknown issuer")
        || lower.contains("invalid peer certificate")
        || lower.contains("badencoding")
        || lower.contains("badsignature")
        || lower.contains("notvalidforname")
        || lower.contains("certificate")
    {
        return LinkError::new(ErrorKind::Cert, "TLS 证书不受信任（可配置 webdav.ca_cert）");
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return LinkError::new(ErrorKind::Timeout, "TLS 握手超时");
    }
    LinkError::new(ErrorKind::Net, format!("TLS 连接失败：{message}"))
}

#[cfg(feature = "tls-rustls")]
fn classify_rustls_error(err: &rustls::Error) -> LinkError {
    match err {
        rustls::Error::InvalidCertificate(cert_error) => match cert_error {
            rustls::CertificateError::Expired | rustls::CertificateError::NotValidYet => {
                LinkError::new(ErrorKind::Clock, "证书有效期校验失败，设备时钟可能不正确")
            }
            _ => LinkError::new(
                ErrorKind::Cert,
                format!("TLS 证书不受信任（{}），可配置 webdav.ca_cert", cert_error),
            ),
        },
        rustls::Error::AlertReceived(alert) => {
            LinkError::new(ErrorKind::Net, format!("TLS 握手被服务器拒绝（{alert:?}）"))
        }
        other => LinkError::new(ErrorKind::Net, format!("TLS 连接失败：{other}")),
    }
}

#[cfg(feature = "tls-rustls")]
fn server_name(host: &str) -> Result<rustls::pki_types::ServerName<'static>> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(rustls::pki_types::ServerName::IpAddress(ip.into()));
    }
    rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| LinkError::config(format!("TLS 主机名无效：{host}")))
}

#[cfg(feature = "tls-rustls")]
fn build_tls_config(
    endpoint: &Endpoint,
    ca_cert: &str,
    insecure: bool,
    label: &str,
    rep: &mut Reporter,
) -> Result<Option<std::sync::Arc<rustls::ClientConfig>>> {
    if !endpoint.tls {
        return Ok(None);
    }
    if insecure {
        rep.warn(format!("已禁用 TLS 证书校验（{label}.insecure=true）"));
        let verifier = std::sync::Arc::new(NoCertificateVerification::new());
        let client = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();
        return Ok(Some(std::sync::Arc::new(client)));
    }
    let mut roots = rustls::RootCertStore::empty();
    let ca_cert = ca_cert.trim();
    if ca_cert.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        rep.info(format!("使用内置根证书（{} 个）", roots.len()));
    } else {
        let certs = load_ca_certs(std::path::Path::new(ca_cert), label)?;
        let total = certs.len();
        for cert in certs {
            roots
                .add(cert)
                .map_err(|err| LinkError::config(format!("{label}.ca_cert 证书无效：{err}")))?;
        }
        rep.info(format!("使用 {label}.ca_cert 中的 {total} 个根证书"));
    }
    let client = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(std::sync::Arc::new(client)))
}

/// Load PEM trust anchors for a target's `ca_cert`.
#[cfg(feature = "tls-rustls")]
pub fn load_ca_certs(
    path: &std::path::Path,
    label: &str,
) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>> {
    let bytes = std::fs::read(path).map_err(|err| {
        LinkError::config(format!(
            "无法读取 {label}.ca_cert {}：{err}",
            path.display()
        ))
    })?;
    let mut reader = std::io::BufReader::new(std::io::Cursor::new(bytes));
    let mut certs = Vec::new();
    for item in rustls_pemfile::certs(&mut reader) {
        let der = item.map_err(|err| {
            LinkError::config(format!(
                "{label}.ca_cert {} 解析失败：{err}",
                path.display()
            ))
        })?;
        certs.push(der);
    }
    if certs.is_empty() {
        return Err(LinkError::config(format!(
            "{label}.ca_cert {} 中未找到 PEM 证书",
            path.display()
        )));
    }
    Ok(certs)
}

/// Debug-only verifier for `webdav.insecure = true`.
#[cfg(feature = "tls-rustls")]
#[derive(Debug)]
struct NoCertificateVerification {
    schemes: Vec<rustls::SignatureScheme>,
}

#[cfg(feature = "tls-rustls")]
impl NoCertificateVerification {
    fn new() -> Self {
        NoCertificateVerification {
            schemes: rustls::crypto::ring::default_provider()
                .signature_verification_algorithms
                .supported_schemes(),
        }
    }
}

#[cfg(feature = "tls-rustls")]
impl rustls::client::danger::ServerCertVerifier for NoCertificateVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.schemes.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_http_and_https_endpoints() {
        let endpoint = Endpoint::parse("http://127.0.0.1:8080/dav").unwrap();
        assert!(!endpoint.tls);
        assert_eq!(endpoint.host, "127.0.0.1");
        assert_eq!(endpoint.port, 8080);
        assert_eq!(endpoint.base_path, "/dav");
        assert_eq!(endpoint.base_url(), "http://127.0.0.1:8080/dav");

        let endpoint = Endpoint::parse("https://nas.example.com").unwrap();
        assert!(endpoint.tls);
        assert_eq!(endpoint.port, 443);
        assert_eq!(endpoint.base_path, "");
        assert_eq!(endpoint.base_url(), "https://nas.example.com");

        let endpoint = Endpoint::parse("HTTPS://NAS.example.com:8443/dav/").unwrap();
        assert!(endpoint.tls);
        assert_eq!(endpoint.host, "NAS.example.com");
        assert_eq!(endpoint.port, 8443);
        assert_eq!(endpoint.base_path, "/dav");
        assert_eq!(endpoint.base_url(), "https://NAS.example.com:8443/dav");

        let endpoint = Endpoint::parse("http://[::1]:9000/dav").unwrap();
        assert_eq!(endpoint.host, "::1");
        assert_eq!(endpoint.port, 9000);
        assert_eq!(endpoint.base_url(), "http://[::1]:9000/dav");
    }

    #[test]
    fn rejects_bad_endpoints() {
        assert_eq!(
            Endpoint::parse("ftp://x").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            Endpoint::parse("http://").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            Endpoint::parse("http://user:pass@x/").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            Endpoint::parse("http://x/?a=b").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            Endpoint::parse("http://x:notaport/").unwrap_err().kind(),
            ErrorKind::Config
        );
        assert_eq!(
            Endpoint::parse("http://x:0/").unwrap_err().kind(),
            ErrorKind::Config
        );
    }

    #[test]
    fn encodes_utf8_path_segments() {
        assert_eq!(
            encode_segment("合金弹头"),
            "%E5%90%88%E9%87%91%E5%BC%B9%E5%A4%B4"
        );
        assert_eq!(encode_segment("Pokemon - Emerald"), "Pokemon%20-%20Emerald");
        assert_eq!(encode_segment("a_b.c~d"), "a_b.c~d");
        assert_eq!(encode_segment(r#"a/b\c:d?e"#), "a%2Fb%5Cc%3Ad%3Fe");
        assert_eq!(
            encode_segments(["GBA", "mgba", "合金弹头"]),
            "GBA/mgba/%E5%90%88%E9%87%91%E5%BC%B9%E5%A4%B4"
        );
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(
            base64_encode("用户:密码".as_bytes()),
            "55So5oi3OuWvhueggQ=="
        );
    }

    #[test]
    fn classifies_tls_error_strings() {
        assert_eq!(
            classify_tls("invalid peer certificate: Expired").kind(),
            ErrorKind::Clock
        );
        assert_eq!(
            classify_tls("invalid peer certificate: NotValidYet").kind(),
            ErrorKind::Clock
        );
        assert_eq!(
            classify_tls("invalid peer certificate: UnknownIssuer").kind(),
            ErrorKind::Cert
        );
        assert_eq!(classify_tls("connection reset").kind(), ErrorKind::Net);
    }

    #[test]
    fn status_errors_map_to_categories() {
        assert_eq!(status_error(401).kind(), ErrorKind::Auth);
        assert_eq!(status_error(403).kind(), ErrorKind::Auth);
        assert_eq!(status_error(500).kind(), ErrorKind::Server);
        assert_eq!(status_error(408).kind(), ErrorKind::Timeout);
        assert_eq!(status_error(302).kind(), ErrorKind::Server);
    }

    #[test]
    fn probe_accepts_a_webdav_multistatus_reply() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0u8; 4096];
            let size = stream.read(&mut request).unwrap();
            let text = String::from_utf8_lossy(&request[..size]);
            assert!(
                text.starts_with("PROPFIND /dav/linkSlot HTTP/1.1"),
                "{text}"
            );
            stream
                .write_all(
                    b"HTTP/1.1 207 Multi-Status\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .unwrap();
        });

        let root = std::env::temp_dir().join(format!(
            "linkslot-probe-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("linkSlot/Config")).unwrap();
        std::fs::write(
            root.join("linkSlot/Config/config.toml"),
            format!(
                "[webdav]\nurl = \"http://127.0.0.1:{port}/dav\"\nremote_root = \"/linkSlot\"\n\n[sync]\ntimeout_secs = 2\nretries = 0\n"
            ),
        )
        .unwrap();
        let config = Config::load(&root, None).unwrap();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut rep = Reporter::new(&root, false, &mut out, &mut err);
        let client = DavClient::new(&config, &mut rep).unwrap();
        assert_eq!(client.probe(&mut rep).unwrap(), 207);
        let _ = std::fs::remove_dir_all(&root);
        server.join().unwrap();
    }

    #[test]
    fn request_target_extraction() {
        assert_eq!(
            request_target("http://127.0.0.1:1/linkSlot/a%20b").unwrap(),
            "/linkSlot/a%20b"
        );
        assert_eq!(request_target("https://host").unwrap(), "/");
    }

    #[cfg(feature = "tls-rustls")]
    #[test]
    fn classifies_rustls_certificate_errors() {
        let expired = rustls::Error::InvalidCertificate(rustls::CertificateError::Expired);
        assert_eq!(classify_rustls_error(&expired).kind(), ErrorKind::Clock);
        let not_yet = rustls::Error::InvalidCertificate(rustls::CertificateError::NotValidYet);
        assert_eq!(classify_rustls_error(&not_yet).kind(), ErrorKind::Clock);
        let unknown = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
        assert_eq!(classify_rustls_error(&unknown).kind(), ErrorKind::Cert);
    }

    #[cfg(feature = "tls-rustls")]
    #[test]
    fn loads_pem_ca_cert_or_errors_cleanly() {
        let root =
            std::env::temp_dir().join(format!("linkslot-pem-{}", crate::util::unique_suffix()));
        std::fs::create_dir_all(&root).unwrap();
        let bad = root.join("bad.pem");
        std::fs::write(&bad, b"not a pem").unwrap();
        let err = load_ca_certs(&bad, "webdav").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Config);
        let missing = root.join("missing.pem");
        assert_eq!(
            load_ca_certs(&missing, "mod").unwrap_err().kind(),
            ErrorKind::Config
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(feature = "tls-rustls")]
    #[test]
    fn builds_tls_config_for_https_and_ignores_http() {
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let mut rep = Reporter::new(std::path::Path::new("/tmp"), false, &mut out, &mut err);

        let https = Endpoint::parse("https://nas.example.com/dav").unwrap();
        let tls = build_tls_config(&https, "", false, "webdav", &mut rep).unwrap();
        assert!(tls.is_some());

        let http = Endpoint::parse("http://127.0.0.1/dav").unwrap();
        let tls = build_tls_config(&http, "", false, "mod", &mut rep).unwrap();
        assert!(tls.is_none());
    }

    // -- config/transport helpers for the tests below -----------------------

    fn test_config_for(port: u16, tag: &str) -> (std::path::PathBuf, Config) {
        let root = std::env::temp_dir().join(format!(
            "linkslot-webdav-{tag}-{}",
            crate::util::unique_suffix()
        ));
        std::fs::create_dir_all(root.join("linkSlot/Config")).unwrap();
        std::fs::write(
            root.join("linkSlot/Config/config.toml"),
            format!(
                "[webdav]\nurl = \"http://127.0.0.1:{port}/dav\"\nremote_root = \"/linkSlot\"\n\n[sync]\ntimeout_secs = 2\nretries = 0\n"
            ),
        )
        .unwrap();
        let config = Config::load(&root, None).unwrap();
        (root, config)
    }

    /// Serve the given responses on sequential connections; returns the port.
    fn spawn_responses(responses: Vec<(u16, Vec<u8>)>) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = std::thread::spawn(move || {
            for (status, body) in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let reason = match status {
                    200 => "OK",
                    207 => "Multi-Status",
                    404 => "Not Found",
                    _ => "Status",
                };
                let head = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
            }
        });
        (port, handle)
    }

    fn with_cleanup<F>(root: &std::path::Path, body: F)
    where
        F: FnOnce(),
    {
        body();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn get_limited_enforces_the_body_cap_and_404_is_none() {
        let (port, server) = spawn_responses(vec![
            (200, b"hello".to_vec()),
            (
                404,
                b"a large error page that is bigger than the cap".to_vec(),
            ),
            (200, b"0123456789".to_vec()),
        ]);
        let (root, config) = test_config_for(port, "get-limited");
        with_cleanup(&root, || {
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            let client = DavClient::new(&config, &mut rep).unwrap();
            let rel = ["compat", "中文.txt"];
            assert_eq!(
                client.get_limited(&mut rep, &rel, 32).unwrap().unwrap(),
                b"hello"
            );
            // A 404 body larger than the cap must still map to None, not an error.
            assert!(client.get_limited(&mut rep, &rel, 4).unwrap().is_none());
            let error = client.get_limited(&mut rep, &rel, 4).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Server);
            assert!(error.message().contains("上限"), "{}", error.message());
        });
        server.join().unwrap();
    }

    #[test]
    fn read_response_enforces_the_optional_body_cap() {
        struct FakeStream {
            input: std::io::Cursor<Vec<u8>>,
        }
        impl Read for FakeStream {
            fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
                self.input.read(buffer)
            }
        }
        impl Write for FakeStream {
            fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
                Ok(buffer.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        fn fake_response(status: &str, headers: &str, body: &[u8]) -> Box<dyn ReadWrite> {
            let mut bytes = format!("HTTP/1.1 {status}\r\n{headers}\r\n").into_bytes();
            bytes.extend_from_slice(body);
            Box::new(FakeStream {
                input: std::io::Cursor::new(bytes),
            })
        }

        // Content-Length larger than the cap fails before reading the body.
        let mut stream = fake_response("200 OK", "Content-Length: 10\r\n", b"0123456789");
        match read_response(&mut stream, Some(4)) {
            Err(ReadError::TooLarge(4)) => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // Without a cap the historical unbounded behavior is unchanged.
        let mut stream = fake_response("200 OK", "Content-Length: 10\r\n", b"0123456789");
        assert_eq!(
            read_response(&mut stream, None).unwrap().body,
            b"0123456789"
        );
        // No Content-Length: the cap is applied while reading.
        let mut stream = fake_response("200 OK", "", b"0123456789");
        match read_response(&mut stream, Some(4)) {
            Err(ReadError::TooLarge(4)) => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
        // Chunked bodies observe the same cap.
        let chunked = b"5\r\nhello\r\n0\r\n\r\n";
        let mut stream = fake_response("200 OK", "Transfer-Encoding: chunked\r\n", chunked);
        match read_response(&mut stream, Some(3)) {
            Err(ReadError::TooLarge(3)) => {}
            other => panic!("expected TooLarge, got {other:?}"),
        }
        let mut stream = fake_response("200 OK", "Transfer-Encoding: chunked\r\n", chunked);
        assert_eq!(read_response(&mut stream, Some(5)).unwrap().body, b"hello");
    }

    // -- multistatus parsing (mods feature) --------------------------------

    #[cfg(feature = "mods")]
    fn compat_self() -> Vec<String> {
        vec![
            "dav".to_string(),
            "linkSlot".to_string(),
            "compat".to_string(),
        ]
    }

    #[cfg(feature = "mods")]
    #[test]
    fn parses_multistatus_absolute_and_relative_hrefs() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<x:multistatus xmlns:x="DAV:">
  <x:response>
    <x:href>http://127.0.0.1:9/dav/linkSlot/compat/</x:href>
    <x:propstat>
      <x:prop><x:resourcetype><x:collection/></x:resourcetype></x:prop>
      <x:status>HTTP/1.1 200 OK</x:status>
    </x:propstat>
  </x:response>
  <x:response>
    <x:href>/dav/linkSlot/compat/%E4%B8%AD%E6%96%87.txt</x:href>
    <x:propstat>
      <x:prop>
        <x:resourcetype/>
        <x:getcontentlength>12</x:getcontentlength>
        <x:getlastmodified>Wed, 21 Oct 2015 07:28:00 GMT</x:getlastmodified>
      </x:prop>
      <x:status>HTTP/1.1 200 OK</x:status>
    </x:propstat>
  </x:response>
  <x:response>
    <x:href>sub/</x:href>
    <x:propstat>
      <x:prop>
        <x:resourcetype><x:collection/></x:resourcetype>
        <x:getcontentlength>99</x:getcontentlength>
      </x:prop>
      <x:status>HTTP/1.1 200 OK</x:status>
    </x:propstat>
  </x:response>
</x:multistatus>"#;
        let entries = list_entries_from_multistatus(xml, &compat_self()).unwrap();
        assert_eq!(entries.len(), 2, "self entry must be skipped: {entries:?}");
        assert_eq!(entries[0].name, "中文.txt");
        assert!(!entries[0].is_dir);
        assert_eq!(entries[0].size, 12);
        assert_eq!(
            entries[0].modified.as_deref(),
            Some("Wed, 21 Oct 2015 07:28:00 GMT")
        );
        assert_eq!(entries[1].name, "sub");
        assert!(entries[1].is_dir);
        assert_eq!(entries[1].size, 0, "directory size must be forced to 0");
    }

    #[cfg(feature = "mods")]
    #[test]
    fn skips_self_entries_with_relative_and_stripped_hrefs() {
        let relative = r#"<multistatus xmlns="DAV:">
  <response>
    <href>./</href>
    <propstat><prop><resourcetype><collection/></resourcetype></prop>
      <status>HTTP/1.1 200 OK</status></propstat>
  </response>
  <response>
    <href>child%20one.txt</href>
    <propstat><prop><resourcetype/><getcontentlength>3</getcontentlength></prop>
      <status>HTTP/1.1 200 OK</status></propstat>
  </response>
</multistatus>"#;
        let entries = list_entries_from_multistatus(relative, &compat_self()).unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].name, "child one.txt");
        assert_eq!(entries[0].size, 3);

        // Servers that strip the endpoint base path from absolute hrefs still
        // have their self entry skipped (and only the self entry).
        let stripped = r#"<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/linkSlot/compat/</D:href></D:response>
  <D:response><D:href>/linkSlot/compat/archive.zip</D:href></D:response>
</D:multistatus>"#;
        let entries = list_entries_from_multistatus(stripped, &compat_self()).unwrap();
        assert_eq!(entries.len(), 1, "{entries:?}");
        assert_eq!(entries[0].name, "archive.zip");
        assert!(!entries[0].is_dir);
    }

    #[cfg(feature = "mods")]
    #[test]
    fn empty_multistatus_and_stat_entries() {
        let self_only = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/dav/linkSlot/compat/</D:href></D:response></D:multistatus>"#;
        assert!(list_entries_from_multistatus(self_only, &compat_self())
            .unwrap()
            .is_empty());
        let stat = stat_entry_from_multistatus(self_only, &compat_self())
            .unwrap()
            .unwrap();
        assert_eq!(stat.name, "compat");
        assert!(stat.is_dir);
        assert_eq!(stat.size, 0);

        let empty = r#"<D:multistatus xmlns:D="DAV:"/>"#;
        assert!(list_entries_from_multistatus(empty, &compat_self())
            .unwrap()
            .is_empty());
        assert!(stat_entry_from_multistatus(empty, &compat_self())
            .unwrap()
            .is_none());
    }

    #[cfg(feature = "mods")]
    #[test]
    fn list_rejects_more_than_the_entry_limit() {
        let mut xml = String::from("<D:multistatus xmlns:D=\"DAV:\">");
        for index in 0..=MAX_LIST_ENTRIES {
            xml.push_str(&format!(
                "<D:response><D:href>/dav/linkSlot/compat/{index}.txt</D:href></D:response>"
            ));
        }
        xml.push_str("</D:multistatus>");
        let error = list_entries_from_multistatus(&xml, &compat_self()).unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Server);
        assert!(error.message().contains("上限"), "{}", error.message());
    }

    #[cfg(feature = "mods")]
    #[test]
    fn list_and_stat_against_a_server() {
        let list_body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response><D:href>/dav/linkSlot/compat/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
  <D:response><D:href>/dav/linkSlot/compat/save%20deck.txt</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>42</D:getcontentlength><D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>
</D:multistatus>"#;
        let stat_body = r#"<D:multistatus xmlns:D="DAV:"><D:response><D:href>/dav/linkSlot/compat/save%20deck.txt</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>42</D:getcontentlength><D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response></D:multistatus>"#;
        let (port, server) = spawn_responses(vec![
            (207, list_body.as_bytes().to_vec()),
            (207, stat_body.as_bytes().to_vec()),
        ]);
        let (root, config) = test_config_for(port, "list-stat");
        with_cleanup(&root, || {
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            let client = DavClient::new(&config, &mut rep).unwrap();
            let entries = client.list(&mut rep, &["compat"]).unwrap().unwrap();
            assert_eq!(entries.len(), 1, "{entries:?}");
            assert_eq!(entries[0].name, "save deck.txt");
            assert_eq!(entries[0].size, 42);
            let entry = client
                .stat(&mut rep, &["compat", "save deck.txt"])
                .unwrap()
                .unwrap();
            assert_eq!(entry.name, "save deck.txt");
            assert!(!entry.is_dir);
            assert_eq!(entry.size, 42);
            assert_eq!(
                entry.modified.as_deref(),
                Some("Mon, 01 Jan 2024 00:00:00 GMT")
            );
        });
        server.join().unwrap();
    }

    #[cfg(feature = "mods")]
    #[test]
    fn list_and_stat_return_none_on_404() {
        let (port, server) =
            spawn_responses(vec![(404, b"missing".to_vec()), (404, b"missing".to_vec())]);
        let (root, config) = test_config_for(port, "list-404");
        with_cleanup(&root, || {
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            let client = DavClient::new(&config, &mut rep).unwrap();
            assert!(client.list(&mut rep, &["missing"]).unwrap().is_none());
            assert!(client
                .stat(&mut rep, &["missing", "x.txt"])
                .unwrap()
                .is_none());
        });
        server.join().unwrap();
    }

    #[cfg(feature = "mods")]
    #[test]
    fn list_rejects_an_oversized_multistatus_body() {
        let mut body = String::from("<D:multistatus xmlns:D=\"DAV:\">");
        while body.len() <= MAX_PROPFIND_BODY {
            body.push_str("<D:response><D:href>/dav/linkSlot/compat/x.txt</D:href></D:response>");
        }
        body.push_str("</D:multistatus>");
        let (port, server) = spawn_responses(vec![(207, body.into_bytes())]);
        let (root, config) = test_config_for(port, "list-oversize");
        with_cleanup(&root, || {
            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let mut rep = Reporter::new(&root, true, &mut out, &mut err);
            let client = DavClient::new(&config, &mut rep).unwrap();
            let error = client.list(&mut rep, &["compat"]).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::Server);
            assert!(error.message().contains("上限"), "{}", error.message());
        });
        server.join().unwrap();
    }
}
