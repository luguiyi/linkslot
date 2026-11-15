//! Serves a mod's own Web UI and the sandbox bridge SDK.
//!
//! The page runs in an `<iframe sandbox="allow-scripts">` with an opaque
//! origin. It cannot call `/api/**`; the parent page exposes only the methods
//! allowed by the bridge in `app.js`.

use crate::web::api::localized_error;
use crate::web::http::{Request, Response};
use crate::web::Shared;

const SDK_JS: &str = r#"(function () {
  'use strict';
  var sequence = 0;
  var pending = Object.create(null);
  var resolveReady;
  var ready = new Promise(function (resolve) { resolveReady = resolve; });
  var info = null;

  function finish(id, ok, data, error) {
    var entry = pending[id];
    if (!entry) return;
    delete pending[id];
    if (ok) entry.resolve(data);
    else entry.reject(new Error(error || (info && info.locale === 'en-US' ? 'Mod API request failed' : 'mod API 调用失败')));
  }

  window.addEventListener('message', function (event) {
    var message = event.data;
    if (!message || message.source !== 'linkslot-host') return;
    if (message.type === 'init') {
      info = message.info || {};
      if (resolveReady) resolveReady(info);
      resolveReady = null;
      return;
    }
    if (message.type === 'response') finish(message.id, message.ok, message.data, message.error);
  });

  window.LinkSlotMod = {
    ready: ready,
    info: function () { return info; },
    request: function (method, params) {
      return new Promise(function (resolve, reject) {
        var id = ++sequence;
        pending[id] = { resolve: resolve, reject: reject };
        try {
          window.parent.postMessage({
            source: 'linkslot-mod',
            type: 'request',
            id: id,
            method: method,
            params: params || {}
          }, '*');
        } catch (error) {
          delete pending[id];
          reject(error);
        }
      });
    }
  };
})();
"#;

pub(crate) fn route(shared: &Shared, request: &Request) -> Response {
    if request.method != crate::web::http::Method::Get
        && request.method != crate::web::http::Method::Head
    {
        return localized_error(shared, 405, "web_api.error.method_not_allowed");
    }
    if request.path == "/mod-ui/sdk.js" {
        return Response::text(200, "application/javascript; charset=utf-8", SDK_JS)
            .with_header("Cache-Control", "no-store");
    }
    let Some(rest) = request.path.strip_prefix("/mod-ui/") else {
        return localized_error(shared, 404, "web_api.error.mod_ui_missing");
    };
    let Some((id, relative)) = rest.split_once('/') else {
        return localized_error(shared, 404, "web_api.error.mod_ui_missing");
    };
    let relative = if relative.is_empty() {
        "web/index.html".to_string()
    } else if relative.starts_with("web/") {
        relative.to_string()
    } else {
        format!("web/{relative}")
    };
    let bytes = match slot_mods::read_ui_asset(&shared.root, id, &relative) {
        Ok(bytes) => bytes,
        Err(slot_mods::ModError::NotFound(message)) => return Response::error(404, message),
        Err(slot_mods::ModError::Invalid(message)) => return Response::error(400, message),
        Err(slot_mods::ModError::TooLarge(message)) => return Response::error(413, message),
        Err(error) => return Response::error(500, error.message().to_string()),
    };
    let content_type = content_type(&relative);
    let mut response = Response::new(200, content_type, bytes);
    response = response.with_header("Cache-Control", "no-store");
    if relative.ends_with(".html") || relative.ends_with(".htm") {
        response = response.with_header(
            "Content-Security-Policy",
            "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-ancestors 'self'",
        );
        response = response.with_header("Referrer-Policy", "no-referrer");
    }
    response
}

fn content_type(path: &str) -> &'static str {    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        "text/html; charset=utf-8"
    } else if lower.ends_with(".js") || lower.ends_with(".mjs") {
        "application/javascript; charset=utf-8"
    } else if lower.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if lower.ends_with(".json") {
        "application/json; charset=utf-8"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".woff2") {
        "font/woff2"
    } else {
        "application/octet-stream"
    }
}

#[cfg(test)]
mod tests {
    use super::SDK_JS;

    #[test]
    fn the_sdk_exposes_the_host_locale() {
        // The parent includes this in the init message; the SDK uses it to
        // choose the language of its own fallback error.
        assert!(SDK_JS.contains("info && info.locale === 'en-US'"));
        assert!(SDK_JS.contains("Mod API request failed"));
        assert!(SDK_JS.contains("info = message.info"));
    }
}
