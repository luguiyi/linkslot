//! Slot's built-in Web management service.
//!
//! A std-only HTTP/1.1 server started in-process by the Slot frontend.  It
//! serves one embedded single-page app (overview / game library / saves / file
//! browser / cloud sync / Lua scripts / Wi-Fi / about) plus a small JSON API so
//! a phone or PC on the same LAN can browse, edit covers and replace ROMs.  There is no
//! authentication by design; the service is only reachable from the home
//! network and all writes additionally require the `X-Slot-Web: 1` header sent
//! by the embedded page.
//!
//! Lifecycle (owned by `App`):
//!
//! ```ignore
//! let mut web = WebServer::start(root)?;   // needs Wi-Fi + IPv4
//! web.set_writable(false);                 // game or sync running
//! // ...
//! web.stop();                              // also happens on Drop
//! ```

mod api;
mod assets;
mod files;
mod gbastation;
mod http;
mod i18n;
mod logs;
mod mods;
mod modui;
mod paths;
mod play;
pub(crate) mod roms;
mod saves;
mod settings;
mod upgrade;
mod util;
mod zip_import;
mod tianma;
mod collection;

#[cfg(test)]
mod tests;

use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use http::{Method, Request, Response};

/// First port tried by [`WebServer::start`].
pub const PORT_START: u16 = 8080;
/// Last port tried by [`WebServer::start`].
pub const PORT_END: u16 = 8089;
/// Connections served concurrently; further clients get a 503.
const MAX_CONNECTIONS: usize = 8;
/// Socket read/write timeout for one request.
const IO_TIMEOUT: Duration = Duration::from_secs(10);

/// State shared by the listener and every connection thread.
pub(crate) struct Shared {
    pub(crate) root: PathBuf,
    pub(crate) writable: AtomicBool,
    /// True while a web-triggered linkSlot sync is running.  The device UI
    /// reads this so it cannot start a game in the middle of a sync.
    pub(crate) busy: Arc<AtomicBool>,
    /// Set when the Web page replaces a ROM or cover.  The shelf re-reads the
    /// library and rebuilds its cart faces when the Web menu closes.
    pub(crate) library_changed: Arc<AtomicBool>,
    stop: AtomicBool,
    active: AtomicUsize,
    pub(crate) started: Instant,
    pub(crate) port: u16,
    pub(crate) url: String,
    /// Lazily resolved `linkSlot version` (2 s cap, cached for the session).
    pub(crate) link_version: OnceLock<String>,
    /// Lazily resolved `mods=1` marker from `linkSlot version`, cached like
    /// `link_version` so the script page never re-spawns the CLI.
    pub(crate) mods_supported: OnceLock<bool>,
    pub(crate) tianma_status: Arc<Mutex<crate::tianma::Status>>,
    pub(crate) tianma_cancel: Arc<AtomicBool>,
    pub(crate) gbastation_status: Arc<Mutex<crate::gbastation::Status>>,
    pub(crate) gbastation_cancel: Arc<AtomicBool>,
    pub(crate) collection_status: Arc<Mutex<crate::collection::Status>>,
    pub(crate) collection_cancel: Arc<AtomicBool>,
    pub(crate) collection_upload: Arc<Mutex<Option<crate::collection::Upload>>>,
    /// Largest request body accepted by the HTTP layer, in bytes.
    pub(crate) upload_limit: AtomicUsize,
    /// Bumped whenever the Web page changes the library, so the browser's
    /// game list can be reused instead of rescanning the card per request.
    pub(crate) library_generation: Arc<AtomicU64>,
    /// `(generation, built_at, json)` of the last `/api/roms` answer.
    pub(crate) library_cache: Mutex<Option<(u64, Instant, Vec<u8>)>>,
    /// `(built_at, network json)`: the Wi-Fi probe forks a script, so page
    /// polls must not reach it every time.
    pub(crate) network_cache: Mutex<Option<(Instant, serde_json::Value)>>,
    /// `(built_at, storage json)`: the saves walk touches the whole card, so
    /// the status header reuses it for a few seconds.
    pub(crate) storage_cache: Mutex<Option<(Instant, serde_json::Value)>>,
}

impl Shared {
    fn new(root: PathBuf, ip: &str, port: u16) -> Arc<Shared> {
        let upload_limit = settings::upload_bytes(&root);
        Arc::new(Shared {
            root,
            writable: AtomicBool::new(true),
            busy: Arc::new(AtomicBool::new(false)),
            library_changed: Arc::new(AtomicBool::new(false)),
            stop: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            started: Instant::now(),
            port,
            url: format!("http://{ip}:{port}"),
            link_version: OnceLock::new(),
            mods_supported: OnceLock::new(),
            tianma_status: Arc::new(Mutex::new(crate::tianma::Status::default())),
            tianma_cancel: Arc::new(AtomicBool::new(false)),
            gbastation_status: Arc::new(Mutex::new(crate::gbastation::Status::default())),
            gbastation_cancel: Arc::new(AtomicBool::new(false)),
            collection_status: Arc::new(Mutex::new(crate::collection::Status::default())),
            collection_cancel: Arc::new(AtomicBool::new(false)),
            collection_upload: Arc::new(Mutex::new(None)),
            upload_limit: AtomicUsize::new(upload_limit),
            library_generation: Arc::new(AtomicU64::new(1)),
            library_cache: Mutex::new(None),
            network_cache: Mutex::new(None),
            storage_cache: Mutex::new(None),
        })
    }

    pub(crate) fn writable(&self) -> bool {
        self.writable.load(Ordering::Relaxed)
    }

    pub(crate) fn touch_library_changed(&self) {
        self.library_changed.store(true, Ordering::Release);
        self.library_generation.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn upload_limit(&self) -> usize {
        self.upload_limit.load(Ordering::Relaxed)
    }

    pub(crate) fn set_upload_limit(&self, bytes: usize) {
        self.upload_limit.store(bytes, Ordering::Relaxed);
    }
}

/// Handle to the running management server.  Dropping it stops the service.
pub struct WebServer {
    shared: Arc<Shared>,
    accept: Option<JoinHandle<()>>,
}

impl WebServer {
    /// Start the server for `root` (the card root).  Fails with a Chinese
    /// message when Wi-Fi is not connected or no port could be bound.
    ///
    /// Ports 8080–8089 are tried in order; the URL uses the device's Wi-Fi
    /// IPv4 address so the UI can show/print a reachable link.
    pub fn start(root: PathBuf) -> Result<WebServer, String> {
        let ip = match crate::wifi_job::status_sync(&root) {
            Ok(crate::wifi_job::WifiStatus::Connected { ip, .. }) if !ip.trim().is_empty() => {
                ip.trim().to_string()
            }
            _ => {
                return Err("请先在 网络设置 → WiFi 连接 中连接 WiFi".to_string());
            }
        };
        Self::bind(root, ip, PORT_START..=PORT_END)
    }

    /// Bind the first free port in `ports` and start the accept thread.
    ///
    /// Kept separate from [`WebServer::start`] so unit tests can serve on
    /// loopback without depending on Wi-Fi.
    fn bind(
        root: PathBuf,
        ip: String,
        ports: std::ops::RangeInclusive<u16>,
    ) -> Result<WebServer, String> {
        let first = *ports.start();
        let last = *ports.end();
        let mut listener = None;
        let mut last_error = String::new();
        for port in ports {
            match TcpListener::bind(("0.0.0.0", port)) {
                Ok(bound) => {
                    listener = Some(bound);
                    break;
                }
                Err(error) => last_error = error.to_string(),
            }
        }
        let listener =
            listener.ok_or_else(|| format!("Web 端口 {first}-{last} 均不可用：{last_error}"))?;
        let port = listener
            .local_addr()
            .map_err(|error| format!("无法读取 Web 监听端口：{error}"))?
            .port();
        let shared = Shared::new(root, &ip, port);
        let accept_shared = shared.clone();
        let accept = std::thread::Builder::new()
            .name("slot-web".to_string())
            .spawn(move || accept_loop(listener, accept_shared))
            .map_err(|error| format!("无法启动 Web 服务线程：{error}"))?;
        Ok(WebServer {
            shared,
            accept: Some(accept),
        })
    }

    /// Base URL shown to the user, e.g. `http://192.168.1.42:8080`.
    pub fn url(&self) -> &str {
        &self.shared.url
    }

    /// Bound TCP port.
    pub fn port(&self) -> u16 {
        self.shared.port
    }

    /// Pause/resume all mutating endpoints (game or sync running).
    pub fn set_writable(&self, writable: bool) {
        self.shared.writable.store(writable, Ordering::Relaxed);
    }

    /// True while the accept thread is alive and [`WebServer::stop`] was not
    /// called.
    pub fn running(&self) -> bool {
        self.accept.is_some() && !self.shared.stop.load(Ordering::Acquire)
    }

    /// True while a web-triggered sync is writing saves.  The device UI uses
    /// this to refuse starting a game mid-sync.
    pub fn busy(&self) -> bool {
        self.shared.busy.load(Ordering::Acquire)
    }

    /// True once since the last call if the Web page replaced a cover or ROM.
    /// The device shelf re-reads the library and rebuilds cart faces.
    pub fn take_library_changed(&self) -> bool {
        self.shared.library_changed.swap(false, Ordering::AcqRel)
    }

    /// Stop accepting connections and join the accept thread.  Idempotent.
    pub fn stop(&mut self) {
        let Some(accept) = self.accept.take() else {
            return;
        };
        self.shared.stop.store(true, Ordering::SeqCst);
        // Wake the blocking `accept()` with a throwaway connection.
        let _ = std::net::TcpStream::connect(("127.0.0.1", self.shared.port));
        let _ = accept.join();
    }
}

impl Drop for WebServer {
    fn drop(&mut self) {
        self.stop();
    }
}

fn accept_loop(listener: TcpListener, shared: Arc<Shared>) {
    loop {
        if shared.stop.load(Ordering::Acquire) {
            break;
        }
        match listener.accept() {
            Ok((mut stream, _peer)) => {
                if shared.active.load(Ordering::Relaxed) >= MAX_CONNECTIONS {
                    // Too many parallel transfers: answer immediately with 503
                    // instead of queueing (the page shows the Chinese message).
                    let _ = stream.set_nonblocking(false);
                    http::write_busy(&mut stream);
                    let _ = std::net::TcpStream::shutdown(&stream, std::net::Shutdown::Both);
                    continue;
                }
                shared.active.fetch_add(1, Ordering::Relaxed);
                let connection_shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name("slot-web-conn".to_string())
                    .spawn(move || {
                        struct ActiveGuard(Arc<Shared>);
                        impl Drop for ActiveGuard {
                            fn drop(&mut self) {
                                self.0.active.fetch_sub(1, Ordering::Relaxed);
                            }
                        }
                        let _guard = ActiveGuard(connection_shared.clone());
                        serve_connection(stream, &connection_shared);
                    });
                if spawned.is_err() {
                    shared.active.fetch_sub(1, Ordering::Relaxed);
                }
            }
            Err(_) => {
                if shared.stop.load(Ordering::Acquire) {
                    break;
                }
                // Transient accept error (emfile, aborted handshake, ...).
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

fn serve_connection(mut stream: std::net::TcpStream, shared: &Arc<Shared>) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_nodelay(true);
    match http::read_request_with_limit(&mut stream, shared.upload_limit()) {
        Ok(request) => {
            let head_only = request.method == Method::Head;
            let response = route(shared, &request);
            let _ = http::write_response(&mut stream, &response, head_only);
        }
        Err(error) => {
            let response = Response::error(error.status, error.message);
            let _ = http::write_response(&mut stream, &response, false);
        }
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn route(shared: &Shared, request: &Request) -> Response {
    localize_error_body(shared, route_inner(shared, request))
}

/// Deep handlers (file browser, ROM editor, mod tooling) still carry a few
/// Chinese detail strings.  In English mode never leak them into the page: keep
/// the status code and replace a CJK `error` with the generic translated one.
fn localize_error_body(shared: &Shared, mut response: Response) -> Response {
    if response.status < 400
        || slot_store::read_language(&shared.root) != slot_i18n::Language::EnUs
    {
        return response;
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&response.body) else {
        return response;
    };
    let Some(error) = value.get("error").and_then(|value| value.as_str()) else {
        return response;
    };
    let has_cjk = error
        .chars()
        .any(|c| ('\u{3000}'..='\u{303f}').contains(&c) || ('\u{4e00}'..='\u{9fff}').contains(&c));
    if !has_cjk {
        return response;
    }
    value["error"] = serde_json::Value::String(slot_i18n::t(
        slot_i18n::Language::EnUs,
        "web_api.error.generic",
    ).to_string());
    if let Ok(body) = serde_json::to_vec(&value) {
        response.body = body;
    }
    response
}

fn route_inner(shared: &Shared, request: &Request) -> Response {
    if request.path == "/api" || request.path.starts_with("/api/") {
        return api::route(shared, request);
    }
    if request.path == "/mod-ui" || request.path.starts_with("/mod-ui/") {
        return modui::route(shared, request);
    }
    match request.method {
        Method::Get | Method::Head => assets::route(&request.path),
        _ => Response::error(405, "不支持的请求方法"),
    }
}

#[cfg(test)]
mod test_support {
    //! Helpers shared by the inline tests in the sibling modules.
    use super::*;

    pub fn test_shared(root: PathBuf) -> Arc<Shared> {
        Shared::new(root, "127.0.0.1", 18080)
    }

    pub fn request(method: Method, path: &str, query: &str) -> Request {
        Request {
            method,
            path: path.to_string(),
            query: query.to_string(),
            headers: Default::default(),
            body: Vec::new(),
        }
    }

    pub fn json_request(method: Method, path: &str, body: serde_json::Value) -> Request {
        let mut request = request(method, path, "");
        request.body = serde_json::to_vec(&body).unwrap();
        request
            .headers
            .insert("content-type".to_string(), "application/json".to_string());
        request
    }

    pub fn with_slot_header(mut request: Request) -> Request {
        request
            .headers
            .insert("x-slot-web".to_string(), "1".to_string());
        request
    }

    /// Parse a JSON response body.
    pub fn body_json(response: &Response) -> serde_json::Value {
        serde_json::from_slice(&response.body).unwrap()
    }
}
