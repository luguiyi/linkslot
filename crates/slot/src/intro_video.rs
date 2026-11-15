//! Tiny H.264-in-MP4 clip loader for the game-intro popup.
//!
//! The device build uses Cisco's OpenH264 (BSD) to decode the first H.264
//! video track and keeps a small, 15 fps RGBA frame strip. Audio is ignored;
//! the UI only needs the moving picture. Non-Linux hosts compile a stub so the
//! normal tests and the Windows development build stay dependency-free.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoFrame {
    pub rgba: Vec<u8>,
    pub duration_ms: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VideoClip {
    pub frames: Vec<VideoFrame>,
    pub width: u32,
    pub height: u32,
}

/// One decoded frame, sent to the frontend as soon as it exists. The frontend
/// uploads it as a texture and starts playback without waiting for the tail of
/// the clip.
#[derive(Debug)]
pub enum VideoEvent {
    Frame {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        duration_ms: u32,
    },
    Done,
}

#[cfg(target_os = "linux")]
mod mp4_bitstream_converter;

#[cfg(target_os = "linux")]
const FRAME_MS: u64 = 66;
#[cfg(target_os = "linux")]
const MAX_FRAMES: usize = 180;

#[cfg(target_os = "linux")]
pub fn decode(path: &std::path::Path) -> Option<VideoClip> {
    let mut frames = Vec::new();
    let mut width = 0;
    let mut height = 0;
    let ok = decode_frames(path, |rgba, w, h, duration_ms| {
        if width == 0 {
            width = w;
            height = h;
        }
        frames.push(VideoFrame { rgba, duration_ms });
        true
    });
    if !ok || frames.is_empty() {
        return None;
    }
    Some(VideoClip {
        frames,
        width,
        height,
    })
}

/// Decode frames incrementally. `on_frame` returns false when the consumer is
/// gone (the popup was closed), which stops the worker early.
#[cfg(target_os = "linux")]
pub fn decode_stream(
    path: &std::path::Path,
    on_frame: impl FnMut(Vec<u8>, u32, u32, u32) -> bool,
) -> bool {
    decode_frames(path, on_frame)
}

#[cfg(not(target_os = "linux"))]
pub fn decode(_path: &std::path::Path) -> Option<VideoClip> {
    None
}

#[cfg(not(target_os = "linux"))]
pub fn decode_stream(
    _path: &std::path::Path,
    _on_frame: impl FnMut(Vec<u8>, u32, u32, u32) -> bool,
) -> bool {
    false
}

#[cfg(target_os = "linux")]
fn decode_frames(
    path: &std::path::Path,
    mut on_frame: impl FnMut(Vec<u8>, u32, u32, u32) -> bool,
) -> bool {
    use std::io::Cursor;

    use mp4::MediaType;
    use openh264::decoder::{Decoder, DecoderConfig, Flush};
    use openh264::formats::YUVSource;
    use openh264::OpenH264API;

    use mp4_bitstream_converter::Mp4BitstreamConverter;

    let Ok(bytes) = std::fs::read(path) else {
        return false;
    };
    let Ok(mut reader) = mp4::Mp4Reader::read_header(Cursor::new(&bytes), bytes.len() as u64) else {
        return false;
    };
    let Some((track_id, width, height, timescale)) = reader
        .tracks()
        .values()
        .find(|track| track.media_type().ok() == Some(MediaType::H264))
        .map(|track| {
            (
                track.track_id(),
                track.width() as u32,
                track.height() as u32,
                track.timescale().max(1),
            )
        })
    else {
        return false;
    };
    if width == 0 || height == 0 {
        return false;
    }
    let Some(track) = reader.tracks().get(&track_id) else {
        return false;
    };
    let sample_count = track.sample_count();
    let Ok(mut converter) = Mp4BitstreamConverter::for_mp4_track(track) else {
        return false;
    };
    let decoder_config = DecoderConfig::new()
        .debug(false)
        .flush_after_decode(Flush::NoFlush);
    let Ok(mut decoder) = Decoder::with_api_config(OpenH264API::from_source(), decoder_config) else {
        return false;
    };

    let mut packet = Vec::new();
    let mut full = Vec::new();
    let mut next_keep_ms = 0u64;
    let mut kept = 0usize;
    let mut stopped = false;
    let mut push = |image: openh264::decoder::DecodedYUV<'_>,
                    pts_ms: u64,
                    next_keep_ms: &mut u64,
                    full: &mut Vec<u8>,
                    kept: &mut usize|
     -> bool {
        if pts_ms < *next_keep_ms || *kept >= MAX_FRAMES {
            return true;
        }
        *next_keep_ms = pts_ms + FRAME_MS;
        let (w, h) = image.dimensions();
        if w == 0 || h == 0 {
            return false;
        }
        full.resize(w * h * 4, 0);
        image.write_rgba8(full);
        let (tw, th) = fit_size(w as u32, h as u32, 260, 220);
        let rgba = scale_rgba(full, w as u32, h as u32, tw, th);
        *kept += 1;
        on_frame(rgba, tw, th, FRAME_MS as u32)
    };

    for index in 1..=sample_count {
        let Some(sample) = reader.read_sample(track_id, index).ok().flatten() else {
            continue;
        };
        let pts_ms = sample.start_time.saturating_mul(1000) / timescale as u64;
        converter.convert_packet(&sample.bytes, &mut packet);
        match decoder.decode(&packet) {
            Ok(Some(image)) => {
                if !push(image, pts_ms, &mut next_keep_ms, &mut full, &mut kept) {
                    stopped = true;
                    break;
                }
            }
            Ok(None) => {}
            Err(error) => eprintln!("slot: intro video decode: {error}"),
        }
        if kept >= MAX_FRAMES {
            break;
        }
    }
    if !stopped && kept < MAX_FRAMES {
        if let Ok(rest) = decoder.flush_remaining() {
            let mut pts = next_keep_ms;
            for image in rest {
                if !push(image, pts, &mut next_keep_ms, &mut full, &mut kept) {
                    stopped = true;
                    break;
                }
                pts += FRAME_MS;
                if kept >= MAX_FRAMES {
                    break;
                }
            }
        }
    }
    let _ = (width, height);
    let _ = stopped;
    kept > 0
}

#[cfg(target_os = "linux")]
fn fit_size(w: u32, h: u32, max_w: u32, max_h: u32) -> (u32, u32) {
    let scale = (max_w as f32 / w as f32).min(max_h as f32 / h as f32);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

#[cfg(target_os = "linux")]
fn scale_rgba(src: &[u8], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<u8> {
    if sw == tw && sh == th {
        return src.to_vec();
    }
    let mut out = vec![0u8; (tw * th * 4) as usize];
    for y in 0..th {
        let sy = y * sh / th;
        for x in 0..tw {
            let sx = x * sw / tw;
            let from = ((sy * sw + sx) * 4) as usize;
            let to = ((y * tw + x) * 4) as usize;
            out[to..to + 4].copy_from_slice(&src[from..from + 4]);
        }
    }
    out
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn decodes_the_bundled_h264_sample() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/carousel.mp4");
        let clip = super::decode(&path).expect("the bundled sample did not decode");
        assert!(!clip.frames.is_empty(), "no video frames");
        assert!(clip.width > 0 && clip.height > 0);
        assert!(clip.frames.iter().all(|frame| frame.duration_ms > 0));
    }

    #[test]
    fn streaming_emits_an_early_frame_and_stops_when_asked() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/carousel.mp4");
        let mut frames = 0usize;
        let ok = super::decode_stream(&path, |_rgba, width, height, duration| {
            assert!(width > 0 && height > 0);
            assert!(duration > 0);
            frames += 1;
            frames < 2
        });
        assert!(ok, "no frame was emitted");
        assert_eq!(frames, 2, "the consumer could not stop the decoder");
    }
}
