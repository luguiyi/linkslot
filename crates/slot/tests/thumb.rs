use slot::thumb;
use slot_retro::{GBA_H, GBA_W};

fn decode(png_bytes: &[u8]) -> (Vec<u8>, u32, u32) {
    let mut reader = png::Decoder::new(png_bytes).read_info().expect("read info");
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("decode");
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buf.truncate(info.buffer_size());
    (buf, info.width, info.height)
}

fn filled(w: u32, h: u32, colour: [u8; 4]) -> Vec<u8> {
    let mut frame = vec![0u8; (w * h * 4) as usize];
    for px in frame.chunks_exact_mut(4) {
        px.copy_from_slice(&colour);
    }
    frame
}

#[test]
fn a_thumbnail_keeps_the_frames_colours() {
    let frame = filled(GBA_W, GBA_H, [0x20, 0x40, 0xd0, 0xff]);
    let encoded = thumb::png(&frame, GBA_W, GBA_H).expect("encode");
    let (rgb, w, h) = decode(&encoded);
    assert_eq!((w, h), (GBA_W, GBA_H));
    assert_eq!(&rgb[..3], &[0xd0, 0x40, 0x20]);
}

/// A Famicom frame is 256x224, and a PNG whose header disagrees with the pixels
/// it holds is a broken file rather than a small one — the size has to be the
/// frame's own, not the Advance's constant.
#[test]
fn a_famicom_thumbnail_is_encoded_at_the_famicom_size() {
    let (w, h) = (256u32, 224u32);
    let frame = filled(w, h, [0x10, 0x80, 0x30, 0xff]);
    let encoded = thumb::png(&frame, w, h).expect("encode");
    let (rgb, ew, eh) = decode(&encoded);
    assert_eq!((ew, eh), (w, h));
    assert_eq!(&rgb[..3], &[0x30, 0x80, 0x10]);
}

#[test]
fn a_short_frame_is_not_a_thumbnail() {
    assert!(thumb::png(&[], GBA_W, GBA_H).is_none());
    assert!(thumb::png(&vec![0u8; (GBA_W * GBA_H * 4) as usize - 4], GBA_W, GBA_H).is_none());
    // A frame the right length for one size but named as another is not one
    // either: the encoder writes the header it was told, so the length has to
    // agree with it.
    assert!(thumb::png(&vec![0u8; (GBA_W * GBA_H * 4) as usize], 256, 224).is_none());
}
