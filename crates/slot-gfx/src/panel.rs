//! The parts of the framebuffer surface that are not the framebuffer.
//!
//! The panel's size and mode are read from text the kernel writes, and an
//! EGL refusal is turned into an error message. None of that needs a Linux
//! framebuffer to exist, so it lives here and is compiled on every platform:
//! the parsing is worth testing wherever the tests run.

use crate::surface::GfxError;

const EGL_SUCCESS: i32 = 0x3000;

/// The panel size from `/sys/class/graphics/fb0/virtual_size`, which reads
/// `width,height`.
pub fn panel_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.trim().split_once(',')?;
    let (w, h) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// The panel mode from `/sys/class/graphics/fb0/mode`, which reads
/// `U:720x480p-59`.
pub fn panel_mode(text: &str) -> Option<(u32, u32)> {
    let body = text.lines().next()?.rsplit(':').next()?;
    let (w, rest) = body.split_once('x')?;
    let h: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let (w, h) = (w.trim().parse().ok()?, h.parse().ok()?);
    (w > 0 && h > 0).then_some((w, h))
}

/// An EGL call that refused. EGL says `EGL_SUCCESS` when it refused without
/// flagging a reason, which is worth saying plainly rather than printing as
/// an error code.
pub fn egl_error(what: &str, code: i32) -> GfxError {
    match code {
        EGL_SUCCESS => GfxError::Context(format!("{what}: refused, egl flagged nothing")),
        _ => GfxError::Context(format!("{what}: egl error 0x{code:x}")),
    }
}
