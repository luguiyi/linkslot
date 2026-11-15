/// A save-state thumbnail, encoded at the frame's own size.
///
/// The picture used to be one fixed 240x160 buffer, so the encoder could name
/// the Advance's size outright. A Famicom frame is 256x224, and a PNG whose
/// header disagrees with its pixels is a broken file rather than a small one,
/// so the size is an argument now.
pub fn png(xrgb8888: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let n = (w as usize).checked_mul(h as usize)?;
    if n == 0 || xrgb8888.len() < n * 4 {
        return None;
    }
    let mut rgb = Vec::with_capacity(n * 3);
    for px in xrgb8888[..n * 4].chunks_exact(4) {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }

    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().ok()?;
    writer.write_image_data(&rgb).ok()?;
    writer.finish().ok()?;
    Some(out)
}
