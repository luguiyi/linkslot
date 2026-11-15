//! Minimal HTTP/1.1 request parser and response writer.
//!
//! The Slot device serves a single-purpose LAN page, so the server implements
//! only what that page needs: one request per connection (`Connection: close`),
//! `Content-Length` bodies, and a hard cap on header/body sizes.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;

use crate::web::util;

/// Request-line + headers may not exceed this many bytes.
pub const MAX_HEADER_BYTES: usize = 16 * 1024;
/// Default per-request upload cap; the Web settings page can raise it.
#[cfg(test)]
pub const DEFAULT_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Absolute in-memory request-body ceiling, regardless of the user setting.
pub const MAX_BODY_BYTES: usize = 256 * 1024 * 1024;
/// A request target longer than this is rejected before decoding.
const MAX_TARGET_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Get,
    Put,
    Post,
    Head,
    Other,
}

impl Method {
    fn parse(token: &str) -> Method {
        match token {
            "GET" => Method::Get,
            "PUT" => Method::Put,
            "POST" => Method::Post,
            "HEAD" => Method::Head,
            _ => Method::Other,
        }
    }
}

#[derive(Debug)]
pub struct Request {
    pub method: Method,
    /// Percent-decoded path with the query stripped, e.g. `/api/status`.
    pub path: String,
    /// Raw query string (still percent-encoded), e.g. `path=States%2F...`.
    pub query: String,
    /// Header names are stored lower-case.
    pub headers: HashMap<String, String>,
    pub body: Vec<u8>,
}

impl Request {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

#[derive(Debug, Clone)]
pub struct HttpError {
    pub status: u16,
    pub message: String,
}

impl HttpError {
    pub fn new(status: u16, message: impl Into<String>) -> HttpError {
        HttpError {
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn new(status: u16, content_type: impl Into<String>, body: Vec<u8>) -> Response {
        Response {
            status,
            content_type: content_type.into(),
            body,
            headers: Vec::new(),
        }
    }

    pub fn text(status: u16, content_type: impl Into<String>, body: impl Into<String>) -> Response {
        Response::new(status, content_type, body.into().into_bytes())
    }

    pub fn json(status: u16, value: serde_json::Value) -> Response {
        let body = serde_json::to_vec(&value)
            .unwrap_or_else(|_| "{\"error\":\"JSON 序列化失败\"}".as_bytes().to_vec());
        Response::new(status, "application/json; charset=utf-8", body)
    }

    /// Uniform JSON error body: `{"error": "..."}`.
    pub fn error(status: u16, message: impl Into<String>) -> Response {
        Response::json(status, serde_json::json!({ "error": message.into() }))
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Response {
        self.headers.push((name.into(), value.into()));
        self
    }
}

/// Parse one request with the default 32 MiB body cap.
#[cfg(test)]
pub fn read_request<R: Read>(reader: &mut R) -> Result<Request, HttpError> {
    read_request_with_limit(reader, DEFAULT_BODY_BYTES)
}

/// Parse one request from `reader` (a `TcpStream` in production, a cursor in
/// unit tests).  `limit` is the configurable upload cap; it is clamped to the
/// process safety ceiling.  The caller is responsible for socket timeouts.
pub fn read_request_with_limit<R: Read>(
    reader: &mut R,
    limit: usize,
) -> Result<Request, HttpError> {
    let limit = limit.clamp(1024 * 1024, MAX_BODY_BYTES);
    let mut buffer: Vec<u8> = Vec::with_capacity(2048);
    let header_end = loop {
        if let Some(position) = find_header_end(&buffer) {
            break position;
        }
        if buffer.len() >= MAX_HEADER_BYTES {
            return Err(HttpError::new(413, "请求头超过 16KB 限制"));
        }
        let mut chunk = [0u8; 4096];
        let read = reader
            .read(&mut chunk)
            .map_err(|_| HttpError::new(400, "读取请求失败"))?;
        if read == 0 {
            return Err(HttpError::new(400, "请求不完整"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > MAX_HEADER_BYTES && find_header_end(&buffer).is_none() {
            return Err(HttpError::new(413, "请求头超过 16KB 限制"));
        }
    };

    let header_bytes = buffer[..header_end].to_vec();
    let trailing = buffer[header_end + 4..].to_vec();
    drop(buffer);

    let head = String::from_utf8(header_bytes)
        .map_err(|_| HttpError::new(400, "请求头不是合法 UTF-8"))?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next().ok_or_else(|| HttpError::new(400, "缺少请求行"))?;
    let mut parts = request_line.split(' ');
    let method = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "缺少请求方法"))?;
    let target = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "缺少请求路径"))?;
    let version = parts
        .next()
        .ok_or_else(|| HttpError::new(400, "缺少 HTTP 版本"))?;
    if parts.next().is_some() {
        return Err(HttpError::new(400, "请求行格式错误"));
    }
    if version != "HTTP/1.1" && version != "HTTP/1.0" {
        return Err(HttpError::new(400, "仅支持 HTTP/1.1"));
    }
    if target.len() > MAX_TARGET_BYTES {
        return Err(HttpError::new(400, request_too_long()));
    }

    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(' ') || line.starts_with('\t') {
            return Err(HttpError::new(400, "不支持折行的请求头"));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(HttpError::new(400, "请求头格式错误"));
        };
        let name = name.trim().to_ascii_lowercase();
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(HttpError::new(400, "请求头名称不合法"));
        }
        headers.insert(name, value.trim().to_string());
    }

    let (raw_path, query) = match target.split_once('?') {
        Some((path, query)) => (path, query),
        None => (target, ""),
    };
    let path = crate::web::paths::percent_decode(raw_path)
        .map_err(|_| HttpError::new(400, "请求路径编码不合法"))?;

    let content_length = match headers.get("content-length") {
        Some(value) => value
            .parse::<usize>()
            .map_err(|_| HttpError::new(400, "Content-Length 不合法"))?,
        None => 0,
    };
    if content_length > limit {
        let mb = (limit / (1024 * 1024)).max(1);
        return Err(HttpError::new(413, format!("上传内容超过 {mb}MB 限制")));
    }
    // `Transfer-Encoding` (chunked) is intentionally unsupported on the LAN page.
    if headers.contains_key("transfer-encoding") {
        return Err(HttpError::new(400, "不支持 Transfer-Encoding"));
    }

    let mut body = Vec::with_capacity(content_length.min(1024 * 1024));
    let take = trailing.len().min(content_length);
    body.extend_from_slice(&trailing[..take]);
    let mut remaining = content_length - take;
    let mut chunk = [0u8; 8192];
    while remaining > 0 {
        let want = remaining.min(chunk.len());
        let read = reader
            .read(&mut chunk[..want])
            .map_err(|_| HttpError::new(400, "读取请求体失败"))?;
        if read == 0 {
            return Err(HttpError::new(400, "请求体不完整"));
        }
        body.extend_from_slice(&chunk[..read]);
        remaining -= read;
    }

    Ok(Request {
        method: Method::parse(method),
        path,
        query: query.to_string(),
        headers,
        body,
    })
}

fn request_too_long() -> &'static str {
    "请求路径过长"
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
}

/// Serialise and write `response` to the socket.  `head_only` drops the body
/// (used for `HEAD` requests).
pub fn write_response(stream: &mut TcpStream, response: &Response, head_only: bool) -> std::io::Result<()> {
    let mut out = Vec::with_capacity(256 + response.body.len());
    out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", response.status, reason(response.status)).as_bytes());
    out.extend_from_slice(format!("Content-Type: {}\r\n", response.content_type).as_bytes());
    out.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n");
    out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
    for (name, value) in &response.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    stream.write_all(&out)?;
    if !head_only {
        stream.write_all(&response.body)?;
    }
    stream.flush()
}

pub fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Error",
    }
}

/// Serialise a response into bytes (test helper).
#[cfg(test)]
pub fn response_bytes(response: &Response, head_only: bool) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(format!("HTTP/1.1 {} {}\r\n", response.status, reason(response.status)).as_bytes());
    out.extend_from_slice(format!("Content-Type: {}\r\n", response.content_type).as_bytes());
    out.extend_from_slice(format!("Content-Length: {}\r\n", response.body.len()).as_bytes());
    out.extend_from_slice(b"Connection: close\r\n");
    out.extend_from_slice(b"X-Content-Type-Options: nosniff\r\n");
    for (name, value) in &response.headers {
        out.extend_from_slice(format!("{name}: {value}\r\n").as_bytes());
    }
    out.extend_from_slice(b"\r\n");
    if !head_only {
        out.extend_from_slice(&response.body);
    }
    out
}

/// Convenience for the connection-limit 503: no body needed by clients.
pub fn write_busy(stream: &mut TcpStream) {
    let response = Response::error(503, "Web 服务连接数过多，请稍后重试");
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(2)));
    let _ = write_response(stream, &response, false);
}

/// Decode the (first) `name` query parameter of a request.
pub fn query_param<'a>(request: &'a Request, name: &str) -> Result<Option<String>, HttpError> {
    for pair in request.query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, raw_value) = match pair.split_once('=') {
            Some((key, value)) => (key, value),
            None => (pair, ""),
        };
        let key = crate::web::paths::percent_decode(key)
            .map_err(|_| HttpError::new(400, "查询参数编码不合法"))?;
        if key != name {
            continue;
        }
        // Query values use form semantics: `+` means a space.
        let form = raw_value.replace('+', " ");
        let value = crate::web::paths::percent_decode(&form)
            .map_err(|_| HttpError::new(400, "查询参数编码不合法"))?;
        return Ok(Some(value));
    }
    Ok(None)
}

/// Extract the body of a JSON request, mapping parse failures to a 400.
pub fn json_body(request: &Request) -> Result<serde_json::Value, HttpError> {
    if request.body.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    let value: serde_json::Value = serde_json::from_slice(&request.body)
        .map_err(|_| HttpError::new(400, "请求 JSON 不合法"))?;
    if !value.is_object() {
        return Err(HttpError::new(400, "请求 JSON 必须是对象"));
    }
    Ok(value)
}

/// Small helper used by API handlers to read a required string field.
pub fn json_string(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|field| field.as_str()).map(str::to_string)
}

/// Small helper used by API handlers to read a boolean field with a default.
pub fn json_bool(value: &serde_json::Value, key: &str, default: bool) -> bool {
    value
        .get(key)
        .and_then(|field| field.as_bool())
        .unwrap_or(default)
}

/// Build the `Content-Disposition` value for a download.
pub fn content_disposition(kind: &str, file_name: &str) -> String {
    format!(
        "{kind}; filename=\"{}\"; filename*=UTF-8''{}",
        util::ascii_filename(file_name),
        util::encode_rfc5987(file_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn parse(raw: &str) -> Result<Request, HttpError> {
        read_request(&mut Cursor::new(raw.as_bytes().to_vec()))
    }

    #[test]
    fn parses_get_with_query() {
        let request = parse("GET /api/status?x=1 HTTP/1.1\r\nHost: slot\r\nX-Slot-Web: 1\r\n\r\n").unwrap();
        assert_eq!(request.method, Method::Get);
        assert_eq!(request.path, "/api/status");
        assert_eq!(request.query, "x=1");
        assert_eq!(request.header("x-slot-web"), Some("1"));
        assert!(request.body.is_empty());
    }

    #[test]
    fn parses_put_body_and_content_length() {
        let request = parse("PUT /api/file?path=a HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello").unwrap();
        assert_eq!(request.method, Method::Put);
        assert_eq!(request.body, b"hello");
    }

    #[test]
    fn rejects_oversized_content_length_with_413() {
        let raw = format!("PUT /api/file HTTP/1.1\r\nContent-Length: {}\r\n\r\n", MAX_BODY_BYTES + 1);
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.status, 413);
    }

    #[test]
    fn rejects_oversized_headers() {
        let mut raw = String::from("GET / HTTP/1.1\r\n");
        while raw.len() < MAX_HEADER_BYTES + 16 {
            raw.push_str("X-Pad: 0123456789012345678901234567890123456789\r\n");
        }
        raw.push_str("\r\n");
        let err = parse(&raw).unwrap_err();
        assert_eq!(err.status, 413);
    }

    #[test]
    fn rejects_bad_request_line() {
        assert_eq!(parse("GET / HTTP/9.9\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("GET\r\n\r\n").unwrap_err().status, 400);
        assert_eq!(parse("").unwrap_err().status, 400);
    }

    #[test]
    fn rejects_truncated_body() {
        let err = parse("PUT /x HTTP/1.1\r\nContent-Length: 10\r\n\r\nshort").unwrap_err();
        assert_eq!(err.status, 400);
    }

    #[test]
    fn rejects_bad_header_name() {
        assert_eq!(parse("GET / HTTP/1.1\r\nBad Header: 1\r\n\r\n").unwrap_err().status, 400);
    }

    #[test]
    fn decodes_query_form_semantics() {
        let request = parse("GET /api/file?path=States%2FGBA%2Fa+b.state HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(
            query_param(&request, "path").unwrap().as_deref(),
            Some("States/GBA/a b.state")
        );
        assert_eq!(query_param(&request, "missing").unwrap(), None);
    }

    #[test]
    fn response_bytes_are_well_formed() {
        let response = Response::error(403, "缺少请求头").with_header("Cache-Control", "no-cache");
        let bytes = response_bytes(&response, false);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 403 Forbidden\r\n"));
        assert!(text.contains("Content-Length: "));
        assert!(text.contains("Connection: close\r\n"));
        assert!(text.ends_with("缺少请求头\"}"));
    }

    #[test]
    fn content_disposition_escapes_chinese() {
        let value = content_disposition("attachment", "宝可梦.sav");
        assert!(value.starts_with("attachment; filename=\""));
        assert!(value.contains("filename*=UTF-8''%E5%AE%9D%E5%8F%AF%E6%A2%A6.sav"));
    }
}
