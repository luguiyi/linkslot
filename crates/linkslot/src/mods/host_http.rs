//! Scoped HTTP host functions for Lua mods.
//!
//! A mod may only reach the origin declared by its own `config.json`
//! `base_url`.  The Lua side passes relative path segments and a query table;
//! this module encodes them, enforces request/byte/time caps and returns the
//! raw status/body.  It intentionally reuses the WebDAV transport so TLS,
//! timeouts and response limits stay in one place.

use crate::error::LinkError;
use crate::mods::{
    MAX_HTTP_HEADERS, MAX_HTTP_HEADER_BYTES, MAX_HTTP_READ, MAX_HTTP_REQUESTS, MAX_HTTP_TOTAL,
    MAX_HTTP_URL, MAX_HTTP_WRITE,
};
use crate::output::Reporter;
use crate::webdav::{
    encode_query_value, encode_segment, DavClient, DavResponse, DavTarget, Endpoint,
};

pub(crate) struct HostHttp {
    client: DavClient,
    base_url: String,
    requests: usize,
    bytes_in: usize,
    bytes_out: usize,
}

impl HostHttp {
    pub(crate) fn new(
        endpoint: Endpoint,
        timeout_secs: u64,
        rep: &mut Reporter,
    ) -> Result<HostHttp, LinkError> {
        let target = DavTarget {
            endpoint: &endpoint,
            remote_root: "",
            username: "",
            password: "",
            ca_cert: "",
            insecure: false,
            retries: 0,
            timeout_secs,
            label: "mods http",
        };
        let base_url = endpoint.base_url();
        let client = DavClient::from_target(&target, rep)?;
        Ok(HostHttp {
            client,
            base_url,
            requests: 0,
            bytes_in: 0,
            bytes_out: 0,
        })
    }

    pub(crate) fn request(
        &mut self,
        method: &str,
        segments: &[String],
        query: &[(String, String)],
        body: Option<&[u8]>,
        content_type: Option<&str>,
        extra_headers: &[(String, String)],
    ) -> Result<DavResponse, String> {
        let method = method.to_ascii_uppercase();
        if !matches!(method.as_str(), "GET" | "POST" | "PUT") {
            return Err(format!("不支持的 HTTP 方法 {method:?}（仅 GET/POST/PUT）"));
        }
        if self.requests >= MAX_HTTP_REQUESTS {
            return Err(format!(
                "HTTP 请求次数超过上限（{MAX_HTTP_REQUESTS} 次/脚本）"
            ));
        }
        let body_len = body.map_or(0, <[u8]>::len);
        if body_len > MAX_HTTP_WRITE {
            return Err(format!(
                "HTTP 请求体超过上限（{} MiB）",
                MAX_HTTP_WRITE / 1024 / 1024
            ));
        }
        if self
            .bytes_in
            .saturating_add(self.bytes_out)
            .saturating_add(body_len)
            > MAX_HTTP_TOTAL
        {
            return Err(format!(
                "HTTP 读写总量超过上限（{} MiB/脚本）",
                MAX_HTTP_TOTAL / 1024 / 1024
            ));
        }
        let url = self.build_url(segments, query)?;
        let extra_headers = validate_headers(extra_headers)?;
        let mut headers: Vec<(&str, String)> = Vec::new();
        if let Some(content_type) = content_type {
            if content_type.contains('\r') || content_type.contains('\n') {
                return Err("Content-Type 含非法字符".to_string());
            }
            headers.push(("Content-Type", content_type.to_string()));
        }
        for (name, value) in extra_headers {
            headers.push((name, value));
        }
        self.requests += 1;
        self.bytes_out = self.bytes_out.saturating_add(body_len);
        let response = self
            .client
            .request_once(&method, &url, &headers, body, Some(MAX_HTTP_READ))
            .map_err(|error| format!("[{}] {}", error.kind().as_str(), error.message()))?;
        self.bytes_in = self.bytes_in.saturating_add(response.body.len());
        if self.bytes_in.saturating_add(self.bytes_out) > MAX_HTTP_TOTAL {
            return Err(format!(
                "HTTP 读写总量超过上限（{} MiB/脚本）",
                MAX_HTTP_TOTAL / 1024 / 1024
            ));
        }
        Ok(response)
    }

    fn build_url(&self, segments: &[String], query: &[(String, String)]) -> Result<String, String> {
        if segments.len() > 32 {
            return Err("HTTP 路径段过多（最多 32 段）".to_string());
        }
        let mut url = self.base_url.clone();
        for segment in segments {
            validate_segment(segment)?;
            url.push('/');
            url.push_str(&encode_segment(segment));
        }
        if !query.is_empty() {
            if query.len() > 32 {
                return Err("HTTP query 参数过多（最多 32 个）".to_string());
            }
            url.push('?');
            for (index, (key, value)) in query.iter().enumerate() {
                if index > 0 {
                    url.push('&');
                }
                url.push_str(&encode_query_value(key));
                url.push('=');
                url.push_str(&encode_query_value(value));
            }
        }
        if url.len() > MAX_HTTP_URL {
            return Err(format!("HTTP URL 过长（上限 {MAX_HTTP_URL} 字节）"));
        }
        Ok(url)
    }
}

fn validate_headers(headers: &[(String, String)]) -> Result<Vec<(&str, String)>, String> {
    if headers.len() > MAX_HTTP_HEADERS {
        return Err(format!("HTTP 请求头过多（最多 {MAX_HTTP_HEADERS} 个）"));
    }
    let mut total = 0usize;
    let mut out = Vec::with_capacity(headers.len());
    for (name, value) in headers {
        if name.is_empty() || name.len() > 128 {
            return Err("HTTP 请求头名称无效".to_string());
        }
        if !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(format!("HTTP 请求头名称无效：{name:?}"));
        }
        if matches!(
            name.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "connection" | "transfer-encoding"
        ) {
            return Err(format!("不允许覆盖 HTTP 请求头 {name:?}"));
        }
        if value.chars().any(char::is_control) {
            return Err(format!("HTTP 请求头 {name:?} 含控制字符"));
        }
        total = total.saturating_add(name.len() + value.len());
        if total > MAX_HTTP_HEADER_BYTES {
            return Err(format!(
                "HTTP 请求头总量超过上限（{} KiB）",
                MAX_HTTP_HEADER_BYTES / 1024
            ));
        }
        out.push((name.as_str(), value.clone()));
    }
    Ok(out)
}

fn validate_segment(segment: &str) -> Result<(), String> {
    if segment.is_empty() {
        return Err("HTTP 路径段不能为空".to_string());
    }
    if segment == "." || segment == ".." {
        return Err("HTTP 路径段不能是 . 或 ..".to_string());
    }
    if segment.chars().any(char::is_control) {
        return Err(format!("HTTP 路径段含控制字符：{segment:?}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostHttp {
        let endpoint = Endpoint::parse("http://127.0.0.1:9").expect("endpoint");
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut rep = Reporter::new(std::path::Path::new("."), false, &mut out, &mut err);
        HostHttp::new(endpoint, 1, &mut rep).expect("host")
    }

    #[test]
    fn path_segments_and_query_are_encoded() {
        let host = host();
        let url = host
            .build_url(
                &[
                    "api".to_string(),
                    "game".to_string(),
                    "sdmc:/rom.gba".to_string(),
                ],
                &[("path".to_string(), "sdmc:/save 1.sav".to_string())],
            )
            .expect("url");
        assert_eq!(
            url,
            "http://127.0.0.1:9/api/game/sdmc%3A%2From.gba?path=sdmc%3A%2Fsave%201.sav"
        );
    }

    #[test]
    fn rejects_dangerous_segments() {
        let host = host();
        for bad in ["", ".", "..", "a\nb"] {
            assert!(
                host.build_url(&[bad.to_string()], &[]).is_err(),
                "accepted {bad:?}"
            );
        }
        // Slashes inside a game id are allowed: they are percent-encoded as
        // part of one segment, exactly like GBAStation's own web client.
        assert!(host.build_url(&["sdmc:/rom.gba".to_string()], &[]).is_ok());
    }

    #[test]
    fn rejects_unknown_methods() {
        let mut host = host();
        assert!(host
            .request("DELETE", &["x".to_string()], &[], None, None, &[])
            .is_err());
    }

    #[test]
    fn accepts_an_authorization_header_but_not_reserved_ones() {
        let auth = [(
            "Authorization".to_string(),
            "Bearer sk-test".to_string(),
        )];
        let good = validate_headers(&auth).expect("bearer header");
        assert_eq!(good[0].0, "Authorization");
        assert_eq!(good[0].1, "Bearer sk-test");

        for name in ["Host", "Content-Length", "Connection", "Transfer-Encoding"] {
            assert!(
                validate_headers(&[(name.to_string(), "x".to_string())]).is_err(),
                "reserved header {name} was accepted"
            );
        }
        assert!(
            validate_headers(&[("X-Test".to_string(), "line\nbreak".to_string())]).is_err(),
            "a header value with a newline was accepted"
        );
    }
}
