//! The picture geometry: which canvas a machine's core draws into, and which
//! slice of it is the machine's own picture.
//!
//! These are the two numbers the whole render stack is a ratio of — the game
//! texture's size, the on-panel rectangle, the shader's `u_pic` gate — so they
//! are pinned here rather than only discovered on a device. The Advance is the
//! reason the numbers used to be a constant; the Famicom is the reason they
//! cannot be.

mod common;

use slot::app::App;
use slot::video_mode::VideoMode;
use slot_store::Platform;

/// Float comparison for a UV coordinate, to four decimal places — the shader
/// takes `f32` and every one of these is an exact ratio of small integers.
fn close(got: [f32; 4], want: [f32; 4]) -> bool {
    got.iter().zip(want.iter()).all(|(g, w)| (g - w).abs() < 1e-4)
}

#[test]
fn each_machine_has_the_canvas_its_cores_draw_into() {
    let d = common::tmp_root_with_carts(&[]);
    let mut app = App::boot(d.path());

    for (platform, want) in [
        // Both handhelds' cores pad or are centred inside 240x160, so the
        // Advance's frame is still the canvas for the whole Game Boy family.
        (Platform::Gba, (240u32, 160u32)),
        (Platform::Gb, (240, 160)),
        (Platform::Gbc, (240, 160)),
        // A Famicom hands over 256x224 once fceumm's overscan is cropped.
        (Platform::Fc, (256, 224)),
        // supafaust hands over the same 8:7 canvas.
        (Platform::Sfc, (256, 224)),
        // Genesis Plus GX hands over 320x224.
        (Platform::Md, (320, 224)),
    ] {
        app.set_platform(platform);
        assert_eq!(
            app.frame_size(),
            want,
            "{} got the wrong canvas",
            platform.name()
        );
    }
}

#[test]
fn the_picture_is_the_whole_canvas_except_on_a_game_boy() {
    let d = common::tmp_root_with_carts(&[]);
    let mut app = App::boot(d.path());

    // The Advance and the Famicom both fill their canvas: the core pads or
    // crops to it, and there is no letterbox to describe.
    for platform in [Platform::Gba, Platform::Fc] {
        app.set_platform(platform);
        assert!(
            close(app.picture_rect(), [0.0, 0.0, 1.0, 1.0]),
            "{} described a letterbox it does not have: {:?}",
            platform.name(),
            app.picture_rect()
        );
    }

    // A Game Boy is 160x144 centred in the 240x160 canvas: 40px of margin each
    // side, 8px top and bottom.
    for platform in [Platform::Gb, Platform::Gbc] {
        app.set_platform(platform);
        assert!(
            close(
                app.picture_rect(),
                [40.0 / 240.0, 8.0 / 160.0, 200.0 / 240.0, 152.0 / 160.0]
            ),
            "{} is not centred in the handheld canvas: {:?}",
            platform.name(),
            app.picture_rect()
        );
    }
}

#[test]
fn a_famicom_has_no_stretch_to_offer_because_its_picture_is_its_canvas() {
    let d = common::tmp_root_with_carts(&[]);
    let mut app = App::boot(d.path());
    app.set_platform(Platform::Fc);

    // "actual" and "stretch" differ only by which slice of the canvas is shown.
    // On a machine whose picture fills the canvas there is no second slice, so
    // both answer the same rectangle — the panel still integer-scales it.
    let actual = app.source_rect();
    app.set_video_mode(slot::video_mode::VideoMode::Stretch);
    let stretch = app.source_rect();
    assert!(
        close(actual, stretch),
        "FC offered two different pictures: {actual:?} vs {stretch:?}"
    );
    assert!(close(actual, [0.0, 0.0, 1.0, 1.0]));
}

#[test]
fn sfc_fc_and_md_fill_the_height_only_in_stretch() {
    let d = common::tmp_root_with_carts(&[]);
    let mut app = App::boot(d.path());

    for platform in [Platform::Sfc, Platform::Fc, Platform::Md] {
        app.set_platform(platform);
        app.set_video_mode(VideoMode::Actual);
        assert!(
            !app.picture_fit(),
            "{} fills by default; the default must stay integer-scaled",
            platform.name()
        );
        assert!(
            close(app.source_rect(), [0.0, 0.0, 1.0, 1.0]),
            "{} should show the whole picture",
            platform.name()
        );

        app.set_video_mode(VideoMode::Stretch);
        assert!(
            app.picture_fit(),
            "{} did not switch to the aspect-preserving fill",
            platform.name()
        );
        assert!(
            close(app.source_rect(), [0.0, 0.0, 1.0, 1.0]),
            "{} cropped the picture instead of filling the height",
            platform.name()
        );
    }

    // The older machines keep their own L1 meaning: GBA has no frontend
    // shoulders at all, and GB/GBC still crop-and-stretch their padded canvas.
    for platform in [Platform::Gba, Platform::Gb, Platform::Gbc] {
        app.set_platform(platform);
        app.set_video_mode(VideoMode::Actual);
        assert!(!app.picture_fit(), "{platform:?} unexpectedly fills");
        app.set_video_mode(VideoMode::Stretch);
        assert!(
            !app.picture_fit(),
            "{platform:?} must keep its old stretch behaviour"
        );
    }
}

#[test]
fn the_sfc_fill_keeps_the_aspect_without_top_or_bottom_bars() {
    let (x, y, w, h) = slot_gfx::base_rect_fit(256, 224);
    assert!((y - 0.0).abs() < 1e-4, "top bar survived: {y}");
    assert!((h - 480.0).abs() < 1e-4, "height is {h}");
    assert!(
        (w / h - 256.0 / 224.0).abs() < 1e-5,
        "fill distorted the SFC picture: {w}x{h}"
    );
    assert!((x - (720.0 - w) / 2.0).abs() < 1e-4, "fill is not centred");
}
