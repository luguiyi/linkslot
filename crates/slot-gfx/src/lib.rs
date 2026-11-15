mod bezel;
mod draw;
#[cfg(unix)]
mod fbdev;
mod fbo;
mod four_pass;
mod gl;
mod grade;
#[cfg(target_os = "macos")]
mod headless;
#[cfg(feature = "host")]
mod host;
mod lcd3x;
mod panel;
mod pipeline;
mod power;
mod quad;
mod shaders;
mod surface;
mod system;

pub use bezel::{framed, lifted_rect, screen_rect};
pub use draw::{Draw, TexId};
#[cfg(unix)]
pub use fbdev::FbdevSurface;
pub use fbo::{canvas_size, Compositor, BACKDROP};
pub use grade::{blue_light_gain, BLUE_LIGHT_MAX};
#[cfg(target_os = "macos")]
pub use headless::HeadlessSurface;
#[cfg(feature = "host")]
pub use host::HostSurface;
pub use lcd3x::{lcd3x_factors, lcd3x_mask, ScreenEffect};
pub use panel::{egl_error, panel_mode, panel_size};
pub use pipeline::WHOLE_TEXTURE;
pub use power::{screen_brightness, screen_scale, screen_width};
pub use surface::{blit_rect, blit_rect_fit, fit_rect, fit_scale, GfxError, Surface, OUT_H, OUT_W};
pub use system::{base_rect, base_rect_fit, scale_for};
