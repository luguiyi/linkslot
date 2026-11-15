use slot_ui::{clock_label, switch_style, ClockPicker, Draw, Field, Language, Polaroids};

#[test]
fn recent_states_read_relative_and_old_ones_read_absolute() {
    let now = "2026-08-09_14-36-05";
    assert_eq!(
        Polaroids::relative_time("2026-08-09_14-32-05", now, Language::ZhCn),
        "4 分钟前"
    );
    assert_eq!(
        Polaroids::relative_time("2026-08-09_03-00-00", now, Language::ZhCn),
        "11 小时前"
    );
    assert_eq!(
        Polaroids::relative_time("2026-08-08_20-00-00", now, Language::ZhCn),
        "2026-08-08 20:00"
    );
    assert_eq!(
        Polaroids::relative_time("2025-01-02_09-05-00", now, Language::ZhCn),
        "2025-01-02 09:05"
    );
}

#[test]
fn the_clock_is_24_hour_and_shows_no_seconds() {
    assert_eq!(
        clock_label("2026-08-09_21-07-00"),
        "21:07",
        "not 24 hour, or showing seconds"
    );
}

#[test]
fn the_clock_picker_is_a_switch_panel_with_a_red_field_underline() {
    let picker = ClockPicker::from_secs(1_767_225_600);
    let mut out = Vec::new();
    picker.draw(None, None, None, Language::ZhCn, &mut out);
    let backdrop = match &out[0] {
        Draw::Rect { colour, .. } => *colour,
        _ => panic!("the picker does not start with a backdrop"),
    };
    let ground = [
        switch_style::bg()[0] as f32 / 255.0,
        switch_style::bg()[1] as f32 / 255.0,
        switch_style::bg()[2] as f32 / 255.0,
        1.0,
    ];
    assert_eq!(backdrop, ground, "the clock panel is not the Switch ground");
    let accent = [
        switch_style::accent()[0] as f32 / 255.0,
        switch_style::accent()[1] as f32 / 255.0,
        switch_style::accent()[2] as f32 / 255.0,
        1.0,
    ];
    assert!(
        out.iter().any(|draw| matches!(
            draw,
            Draw::Rect { colour, .. } if *colour == accent
        )),
        "the selected field has no red underline"
    );
}

#[test]
fn the_clock_fields_are_rounded_cards_with_white_digits() {
    let mut picker = ClockPicker::from_secs(1_767_225_600);
    let face = picker.face();
    assert_eq!(face.h, 44);
    assert!(face.w > 500 && face.w < 720, "odd picker width {}", face.w);
    let alpha = |x: u32, y: u32| face.rgba[((y * face.w + x) * 4 + 3) as usize];
    assert_eq!(alpha(0, 0), 0, "the field row has no transparent corner");
    assert!(alpha(30, 22) > 200, "the first field card is not opaque");
    let is_white = |x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        face.rgba[i + 3] > 200
            && face.rgba[i] == switch_style::ink()[0]
            && face.rgba[i + 1] == switch_style::ink()[1]
            && face.rgba[i + 2] == switch_style::ink()[2]
    };
    assert!(
        (8..80).any(|x| (8..36).any(|y| is_white(x, y))),
        "the year is not printed in white"
    );

    // Moving the selection only changes the underline drawn by `draw`, not the
    // field cards.
    picker.field(Field::Hour);
    assert_eq!(picker.face().rgba, face.rgba, "the cards follow the cursor");
    let mut out = Vec::new();
    picker.draw(None, None, None, Language::ZhCn, &mut out);
    let underline = out.iter().find_map(|draw| match draw {
        Draw::Rect { x, w, colour, .. }
            if *colour
                == [
                    switch_style::accent()[0] as f32 / 255.0,
                    switch_style::accent()[1] as f32 / 255.0,
                    switch_style::accent()[2] as f32 / 255.0,
                    1.0,
                ] =>
        {
            Some((*x, *w))
        }
        _ => None,
    });
    let (x, w) = underline.expect("no red underline");
    assert!(w > 0.0 && x > 0.0);
}

#[test]
fn zz_render_wordmark_variants() {
    let base = include_str!("../assets/wordmark.svg");
    for (name, fill) in [
        ("as-is", "#231f20"),
        ("white", "#f4f4f4"),
        ("red", "#e60012"),
        ("grey", "#9a9aa4"),
    ] {
        let svg = base.replace("#231f20", fill);
        let tree = usvg::Tree::from_str(&svg, &usvg::Options::default()).unwrap();
        let size = tree.size();
        let width = 1400u32;
        let height = (width as f32 * size.height() / size.width()).round() as u32;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).unwrap();
        let transform = resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let mut rgba = pixmap.data().to_vec();
        for px in rgba.chunks_mut(4) {
            let a = px[3] as f32 / 255.0;
            if a > 0.0 {
                for channel in px.iter_mut().take(3) {
                    *channel = ((*channel as f32 / a).round()).min(255.0) as u8;
                }
            }
        }
        let path = std::env::temp_dir().join(format!("wm-{name}.png"));
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()
            .unwrap()
            .write_image_data(&rgba)
            .unwrap();
        eprintln!("wrote {} {}x{}", path.display(), width, height);
    }
}
