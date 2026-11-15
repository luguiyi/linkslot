use crate::lcd3x::ScreenEffect;
use crate::power::screen_brightness;
use crate::quad::Quad;
use crate::shaders::{GAME_FRAG, RECT_VERT};
use crate::surface::{GfxError, OUT_H, OUT_W};

pub const WHOLE_TEXTURE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// set_system 之前的默认画布：GBA 的 240x160，让还没接线的前端保持原样。
const DEFAULT_SRC_W: u32 = 240;
const DEFAULT_SRC_H: u32 = 160;

pub struct GamePass {
    four_pass: std::cell::OnceCell<Result<crate::four_pass::FourPass, String>>,
    prog: gl::types::GLuint,
    game: gl::types::GLuint,
    src_w: u32,
    src_h: u32,
    scale: u32,
    u_rect: gl::types::GLint,
    u_bright: gl::types::GLint,
    u_uv: gl::types::GLint,
    u_mode: gl::types::GLint,
    u_paper_size: gl::types::GLint,
    u_fbo: gl::types::GLint,
    fbo_scale: [f32; 2],
    u_pic: gl::types::GLint,
    pic: [f32; 4],
    paper: gl::types::GLuint,
    paper_size: f32,
    effect: ScreenEffect,
    power: f32,
    src: [f32; 4],
    fit: bool,
}

impl GamePass {
    pub fn new() -> Result<Self, GfxError> {
        let prog = crate::shaders::program(RECT_VERT, GAME_FRAG)?;
        let (src_w, src_h) = (DEFAULT_SRC_W, DEFAULT_SRC_H);
        let game = crate::gl::texture(src_w, src_h, gl::NEAREST, gl::CLAMP_TO_EDGE, gl::BGRA, None);
        let paper = crate::gl::texture(
            1,
            1,
            gl::NEAREST,
            gl::REPEAT,
            crate::gl::gray_format(),
            Some(&[255]),
        );
        let (u_rect, u_bright, u_uv, u_mode, u_paper_size, u_pic, u_fbo);
        unsafe {
            gl::UseProgram(prog);
            gl::Uniform1i(crate::gl::uniform_location(prog, "u_game"), 0);
            gl::Uniform2f(
                crate::gl::uniform_location(prog, "u_src"),
                src_w as f32,
                src_h as f32,
            );
            gl::Uniform2f(
                crate::gl::uniform_location(prog, "u_target"),
                OUT_W as f32,
                OUT_H as f32,
            );
            u_rect = crate::gl::uniform_location(prog, "u_rect");
            u_bright = crate::gl::uniform_location(prog, "u_bright");
            u_uv = crate::gl::uniform_location(prog, "u_uv");
            gl::Uniform1i(crate::gl::uniform_location(prog, "u_paper"), 1);
            u_mode = crate::gl::uniform_location(prog, "u_mode");
            u_paper_size = crate::gl::uniform_location(prog, "u_paper_size");
            u_fbo = crate::gl::uniform_location(prog, "u_fbo");
            u_pic = crate::gl::uniform_location(prog, "u_pic");
        }
        Ok(GamePass {
            four_pass: std::cell::OnceCell::new(),
            prog,
            game,
            src_w,
            src_h,
            scale: crate::system::scale_for(src_w, src_h),
            u_rect,
            u_bright,
            u_uv,
            u_mode,
            u_paper_size,
            u_fbo,
            fbo_scale: [1.0, 1.0],
            u_pic,
            pic: WHOLE_TEXTURE,
            paper,
            paper_size: 1.0,
            effect: ScreenEffect::Lcd3x,
            power: 1.0,
            src: WHOLE_TEXTURE,
            fit: false,
        })
    }

    /// 换机种：按新画布重建游戏纹理。重复传同一尺寸是空操作，
    /// 免得每帧调用都重分配纹理。
    pub fn set_system(&mut self, w: u32, h: u32) {
        // 画布还没定下来（比如刚进前端）时保持现状，别建 0 尺寸纹理。
        if w == 0 || h == 0 || (w == self.src_w && h == self.src_h) {
            return;
        }
        let game = crate::gl::texture(w, h, gl::NEAREST, gl::CLAMP_TO_EDGE, gl::BGRA, None);
        unsafe { gl::DeleteTextures(1, &self.game) };
        self.game = game;
        self.src_w = w;
        self.src_h = h;
        self.scale = crate::system::scale_for(w, h);
        // four-pass 的中间帧缓冲也是按旧画布建的，必须让它下次按新尺寸重建，
        // 否则新机种会被裁进旧 FBO。
        self.four_pass = std::cell::OnceCell::new();
        unsafe {
            gl::UseProgram(self.prog);
            gl::Uniform2f(
                crate::gl::uniform_location(self.prog, "u_src"),
                w as f32,
                h as f32,
            );
        }
    }

    pub fn set_power(&mut self, t: f32) {
        self.power = t.clamp(0.0, 1.0);
    }

    pub fn set_picture(&mut self, rect: [f32; 4]) {
        self.pic = rect;
    }

    pub fn set_fbo_scale(&mut self, x: f32, y: f32) {
        self.fbo_scale = [x, y];
    }

    pub fn set_effect(&mut self, effect: ScreenEffect) {
        self.effect = effect;
    }

    pub fn set_paper(&mut self, size: u32, gray: &[u8]) {
        let tex = crate::gl::texture(
            size,
            size,
            gl::NEAREST,
            gl::REPEAT,
            crate::gl::gray_format(),
            Some(gray),
        );
        unsafe { gl::DeleteTextures(1, &self.paper) };
        self.paper = tex;
        self.paper_size = size as f32;
    }

    pub fn set_source_rect(&mut self, rect: [f32; 4]) {
        self.src = rect;
    }

    /// Keep the source aspect while filling the screen height (SFC/FC L1).
    pub fn set_fit(&mut self, fit: bool) {
        self.fit = fit;
    }

    /// 上载一帧 xrgb8888：h 行、每行 w 个像素、行内紧排（stride = w * 4）。
    pub fn upload(&mut self, xrgb8888: &[u8], w: u32, h: u32) {
        if w == 0 || h == 0 || xrgb8888.len() < w as usize * h as usize * 4 {
            return;
        }
        // 这一帧的尺寸和纹理对不上，说明换机种的 set_system 还没走到；
        // 先按这一帧重建，否则 TexSubImage2D 会越界而整帧丢掉。
        if w != self.src_w || h != self.src_h {
            self.set_system(w, h);
        }
        unsafe {
            gl::BindTexture(gl::TEXTURE_2D, self.game);
            gl::PixelStorei(gl::UNPACK_ALIGNMENT, 1);
            gl::TexSubImage2D(
                gl::TEXTURE_2D,
                0,
                0,
                0,
                w as i32,
                h as i32,
                gl::BGRA,
                gl::UNSIGNED_BYTE,
                xrgb8888.as_ptr() as *const std::ffi::c_void,
            );
        }
    }

    pub fn draw(&self, quad: &Quad) {
        let src = match self.effect {
            // Grid 的整像素对齐只在整数缩放时成立；分数铺满时保留整幅，
            // 否则会被整数 scale 裁掉左右边缘。
            ScreenEffect::Grid if !self.fit => self.whole_scale_source(),
            _ => self.src,
        };
        self.draw_source(self.game, quad, src);
    }

    /// 栅格线要落在输出像素的整边界上：画出来的那一块若不是整数倍，
    /// 就从源矩形两侧各裁掉一点，GBA 的 3 倍铺满画布时不会触发裁剪。
    fn whole_scale_source(&self) -> [f32; 4] {
        let out_w = (self.src_w * self.scale) as f32 * self.fbo_scale[0];
        let out_h = (self.src_h * self.scale) as f32 * self.fbo_scale[1];
        let (x, w) = whole_scale(self.src[0], self.src[2], self.src_w as f32, out_w);
        let (y, h) = whole_scale(self.src[1], self.src[3], self.src_h as f32, out_h);
        [x, y, w, h]
    }

    pub fn draw_still(&self, tex: gl::types::GLuint, quad: &Quad) {
        self.draw_source(tex, quad, WHOLE_TEXTURE);
    }

    fn draw_source(&self, tex: gl::types::GLuint, quad: &Quad, src: [f32; 4]) {
        let (x, y, w, h) = if self.fit {
            crate::power::screen_rect_fit(self.src_w, self.src_h, self.power)
        } else {
            crate::power::screen_rect(self.src_w, self.src_h, self.power)
        };
        if self.effect == ScreenEffect::FourPass && self.power > 0.0 {
            let chain = self.four_pass.get_or_init(|| {
                eprintln!("slot: four-pass: initializing on first use");
                crate::four_pass::FourPass::new(self.src_w, self.src_h).map_err(|err| {
                    let message = err.to_string();
                    eprintln!("slot: four-pass: unavailable; using unfiltered picture: {message}");
                    message
                })
            });
            if let Ok(chain) = chain {
                chain.draw(tex, src, [x,y,w,h], screen_brightness(self.power));
                return;
            }
        }
        unsafe {
            gl::UseProgram(self.prog);
            gl::Uniform4f(self.u_rect, x, y, w, h);
            gl::Uniform4f(self.u_uv, src[0], src[1], src[2], src[3]);
            gl::Uniform1f(self.u_bright, screen_brightness(self.power));
            let mode = if self.effect == ScreenEffect::FourPass { 0.0 } else { self.effect.mode() };
            gl::Uniform1f(self.u_mode, mode);
            gl::Uniform1f(self.u_paper_size, self.paper_size);
            gl::Uniform2f(self.u_fbo, self.fbo_scale[0], self.fbo_scale[1]);
            gl::Uniform4f(
                self.u_pic,
                self.pic[0],
                self.pic[1],
                self.pic[2],
                self.pic[3],
            );
            gl::ActiveTexture(gl::TEXTURE1);
            gl::BindTexture(gl::TEXTURE_2D, self.paper);
            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, tex);
        }
        quad.draw();
    }
}

fn whole_scale(at: f32, len: f32, texels: f32, out: f32) -> (f32, f32) {
    let shown = len * texels;
    let scale = (out / shown).ceil();
    let trim = scale * shown - out;
    if trim >= 0.1 * shown {
        return (at, len);
    }
    let top = (trim / 2.0).floor();
    (at + top / scale / texels, out / scale / texels)
}

impl Drop for GamePass {
    fn drop(&mut self) {
        unsafe {
            gl::DeleteTextures(1, &self.game);
            gl::DeleteTextures(1, &self.paper);
            gl::DeleteProgram(self.prog);
        }
    }
}
