use serde_json::json;

use crate::web::http::{Request, Response};
use crate::web::Shared;

/// Upload and validate a version zip.  Applying it is intentionally not a Web
/// action: the device menu calls `crate::upgrade::apply` after physical input.
pub(super) fn upload(shared: &Shared, request: &Request) -> Response {
    let handled = (|| -> Result<Response, Response> {
        super::api::guard_write(shared, request)?;
        if request.body.len() > shared.upload_limit() {
            let mb = (shared.upload_limit() / (1024 * 1024)).max(1);
            return Err(Response::error(413, format!("升级包超过 {mb}MB 限制")));
        }
        let status = crate::upgrade::stage(&shared.root, &request.body)
            .map_err(|message| Response::error(400, message))?;
        Ok(Response::json(
            200,
            json!({
                "ok": true,
                "current": slot_ui::PROJECT_VERSION,
                "target": status.target,
                "files": status.files,
                "bytes": status.bytes,
                "build": status.build,
                "message": status.message,
            }),
        ))
    })();
    handled.unwrap_or_else(|response| response)
}

pub(super) fn status(shared: &Shared) -> Response {
    Response::json(200, crate::upgrade::status_value(&shared.root))
}
