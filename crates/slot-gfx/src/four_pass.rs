//! Fixed RetroArch-compatible four-pass preset. Default scale means input size
//! for intermediate passes and viewport size for the last pass. Filtering
//! inherits Slot's nearest-neighbour default; no per-pass override is applied.
use crate::gl::{texture, uniform_location};
use crate::surface::{GfxError, OUT_H, OUT_W};

const SOURCES: [&str; 4] = [
    include_str!("../assets/four-pass/gba-color.glsl"),
    include_str!("../assets/four-pass/nds-color.glsl"),
    include_str!("../assets/four-pass/zfast_lcd.glsl"),
    include_str!("../assets/four-pass/lcd1x.glsl"),
];
/// Drop RetroArch-only preprocessor lines (`#pragma name/format/parameter…`).
/// Their values reach the shaders as uniforms through [`PARAMETERS`].
pub fn strip_pragmas(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.lines().any(|line| line.trim_start().starts_with("#pragma")) {
        return std::borrow::Cow::Borrowed(source);
    }
    let mut out = String::with_capacity(source.len());
    for line in source.split_inclusive('\n') {
        if line.trim_start().starts_with("#pragma") {
            continue;
        }
        out.push_str(line);
    }
    std::borrow::Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RetroArch's `#pragma parameter "…"` lines are not GLSL and used to make
    /// every profile fall back to an unfiltered picture.
    #[test]
    fn presets_are_stripped_of_retroarch_pragmas() {
        let mut stripped_any = false;
        for source in SOURCES {
            let cleaned = strip_pragmas(source);
            assert!(
                !cleaned.lines().any(|line| line.trim_start().starts_with("#pragma")),
                "a pragma line survived"
            );
            let expected: String = source
                .split_inclusive('\n')
                .filter(|line| !line.trim_start().starts_with("#pragma"))
                .collect();
            assert_eq!(cleaned.as_ref(), expected, "the shader body changed");
            if cleaned.len() != source.len() {
                stripped_any = true;
            }
        }
        assert!(stripped_any, "no preset actually carried a pragma");
    }

    #[test]
    fn sources_without_pragmas_are_borrowed_untouched() {
        let clean = "#version 100\nvoid main() {}\n";
        assert!(matches!(strip_pragmas(clean), std::borrow::Cow::Borrowed(_)));
    }
}

pub const PARAMETERS: [(&str, f32); 5] = [
    ("darken_screen", 1.0), ("BORDERMULT", 10.0), ("GBAGAMMA", 0.0),
    ("BRIGHTEN_SCANLINES", 12.0), ("BRIGHTEN_LCD", 5.0),
];

pub struct FourPass {
    programs: Vec<u32>,
    textures: [u32; 2],
    fbos: [u32; 2],
    vbo: u32,
    vao: u32,
    size: (u32, u32),
}

impl FourPass {
    pub fn new(w: u32, h: u32) -> Result<Self, GfxError> {
        #[cfg(feature = "test-support")]
        if std::env::var_os("SLOT_TEST_FOUR_PASS_ERROR").is_some() {
            return Err(GfxError::Shader("injected QA failure".into()));
        }
        let mut this = Self { programs: Vec::new(), textures: [0;2], fbos: [0;2], vbo: 0, vao: 0, size: (w, h) };
        for src in SOURCES {
            // The presets are written for RetroArch's shader preprocessor: its
            // `#pragma parameter "…"` lines are not GLSL, and a driver rejects
            // the quotes ("Unknown character '"'(34)"), which used to make the
            // whole four-pass filter fall back to an unfiltered picture.
            let src = strip_pragmas(src);
            // Upstream zfast uses legacy varying for invSize on desktop too.
            let src = src.replace("varying COMPAT_PRECISION vec2 invSize;", "COMPAT_VARYING COMPAT_PRECISION vec2 invSize;");
            let version = if crate::gl::es() { "#version 100\n" } else { "#version 330 core\n" };
            let vert = format!("{version}#define VERTEX\n#define PARAMETER_UNIFORM\n{src}");
            let frag = format!("{version}#define FRAGMENT\n#define PARAMETER_UNIFORM\n#define main retro_main\n{src}\n#undef main\nuniform float slot_brightness;\nvoid main() {{ retro_main(); FragColor = vec4(FragColor.rgb * slot_brightness, 1.0); }}\n");
            this.programs.push(crate::gl::program(&vert, &frag)?);
        }
        unsafe {
            let mut previous = 0;
            gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut previous);
            gl::GenFramebuffers(2, this.fbos.as_mut_ptr());
            for i in 0..2 {
                this.textures[i] = texture(w, h, gl::NEAREST, gl::CLAMP_TO_EDGE, gl::RGBA, None);
                gl::BindFramebuffer(gl::FRAMEBUFFER, this.fbos[i]);
                gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, this.textures[i], 0);
                let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
                if status != gl::FRAMEBUFFER_COMPLETE {
                    gl::BindFramebuffer(gl::FRAMEBUFFER, previous as u32);
                    return Err(GfxError::Framebuffer(status));
                }
            }
            gl::BindFramebuffer(gl::FRAMEBUFFER, previous as u32);
            gl::GenBuffers(1, &mut this.vbo);
            if !crate::gl::es() { gl::GenVertexArrays(1, &mut this.vao); }
        }
        Ok(this)
    }

    pub fn draw(&self, input: u32, src: [f32;4], rect: [f32;4], brightness: f32) {
        let (src_w, src_h) = self.size;
        unsafe {
            let mut target = 0;
            let mut viewport = [0;4];
            let mut old_buffer = 0;
            let mut old_vao = 0;
            gl::GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut target);
            gl::GetIntegerv(gl::VIEWPORT, viewport.as_mut_ptr());
            gl::GetIntegerv(gl::ARRAY_BUFFER_BINDING, &mut old_buffer);
            if !crate::gl::es() { gl::GetIntegerv(gl::VERTEX_ARRAY_BINDING, &mut old_vao); gl::BindVertexArray(self.vao); }
            let blend = gl::IsEnabled(gl::BLEND) == gl::TRUE;
            gl::Disable(gl::BLEND);
            gl::ActiveTexture(gl::TEXTURE0);
            let mut tex = input;
            for (i, &p) in self.programs.iter().enumerate() {
                let last = i == 3;
                let uv = if last { src } else { [0.0, 0.0, 1.0, 1.0] };
                let (output, matrix) = if last {
                    gl::BindFramebuffer(gl::FRAMEBUFFER, target as u32);
                    gl::Viewport(viewport[0], viewport[1], viewport[2], viewport[3]);
                    let [x,y,w,h] = rect;
                    ([w * viewport[2] as f32 / OUT_W as f32, h * viewport[3] as f32 / OUT_H as f32],
                     [2.0*w/OUT_W as f32,0.0,0.0,0.0, 0.0,-2.0*h/OUT_H as f32,0.0,0.0, 0.0,0.0,1.0,0.0, 2.0*x/OUT_W as f32-1.0,1.0-2.0*y/OUT_H as f32,0.0,1.0])
                } else {
                    gl::BindFramebuffer(gl::FRAMEBUFFER, self.fbos[i % 2]);
                    gl::Viewport(0, 0, src_w as i32, src_h as i32);
                    ([src_w as f32,src_h as f32], [2.0,0.0,0.0,0.0, 0.0,2.0,0.0,0.0, 0.0,0.0,1.0,0.0, -1.0,-1.0,0.0,1.0])
                };
                gl::UseProgram(p);
                gl::Uniform1i(uniform_location(p,"Texture"), 0);
                gl::Uniform1i(uniform_location(p,"FrameDirection"), 1);
                gl::Uniform1i(uniform_location(p,"FrameCount"), 0);
                gl::Uniform1f(uniform_location(p,"slot_brightness"), if last {brightness} else {1.0});
                for name in ["TextureSize", "InputSize", "OrigInputSize"] { gl::Uniform2f(uniform_location(p,name), src_w as f32, src_h as f32); }
                gl::Uniform2f(uniform_location(p,"OutputSize"), output[0],output[1]);
                for (name,value) in PARAMETERS { gl::Uniform1f(uniform_location(p,name),value); }
                gl::UniformMatrix4fv(uniform_location(p,"MVPMatrix"),1,gl::FALSE,matrix.as_ptr());
                gl::BindTexture(gl::TEXTURE_2D,tex);
                let [u,v,w,h]=uv;
                let vertices: [f32;16]=[0.0,0.0,u,v, 1.0,0.0,u+w,v, 0.0,1.0,u,v+h, 1.0,1.0,u+w,v+h];
                gl::BindBuffer(gl::ARRAY_BUFFER,self.vbo);
                gl::BufferData(gl::ARRAY_BUFFER,std::mem::size_of_val(&vertices) as isize,vertices.as_ptr().cast(),gl::STREAM_DRAW);
                gl::EnableVertexAttribArray(0);
                gl::EnableVertexAttribArray(1);
                gl::VertexAttribPointer(0,2,gl::FLOAT,gl::FALSE,16,std::ptr::null());
                gl::VertexAttribPointer(1,2,gl::FLOAT,gl::FALSE,16,8usize as *const _);
                gl::DisableVertexAttribArray(2);
                gl::VertexAttrib4f(2,1.0,1.0,1.0,1.0);
                gl::DrawArrays(gl::TRIANGLE_STRIP,0,4);
                if !last { tex=self.textures[i % 2]; }
            }
            gl::DisableVertexAttribArray(1);
            if blend { gl::Enable(gl::BLEND); }
            if !crate::gl::es() { gl::BindVertexArray(old_vao as u32); }
            gl::BindBuffer(gl::ARRAY_BUFFER,old_buffer as u32);
        }
    }
}

impl Drop for FourPass {
    fn drop(&mut self) {
        unsafe {
            for &p in &self.programs { gl::DeleteProgram(p); }
            gl::DeleteTextures(2,self.textures.as_ptr());
            gl::DeleteFramebuffers(2,self.fbos.as_ptr());
            gl::DeleteBuffers(1,&self.vbo);
            if self.vao != 0 { gl::DeleteVertexArrays(1,&self.vao); }
        }
    }
}
