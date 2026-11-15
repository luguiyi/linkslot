use std::path::PathBuf;

use slot_store::{scan, Cart, CartModel, Outline, Platform, ShellChoice, ShellFinish};
use slot_ui::{
    cart_box, cart_face, cart_face_with, clean_label, fc_label_panel, foot_y, gb_label_panel,
    gb_silhouette, label_size,
    label_colour, label_panel, label_text, labelled_cart_face, labelled_cart_shadow, rest_y,
    seated_box, shell_for, silhouette, Finish, GbShell, CART_H, CART_W, DEFAULT_SHELL, FC_BASE_H,
    FC_BASE_W, FC_CART_H,
    FC_CART_W, GB_CART_H, GB_CART_W, GB_LABEL_H, GB_LABEL_W, GB_LABEL_X, GB_LABEL_Y, LABEL_H,
    LABEL_W, LABEL_X, LABEL_Y, MOUTH_H, OUT_H, PLATE_H,
};
use tempfile::TempDir;

fn gx(v: u32) -> u32 {
    let w = seated_box(Platform::Gb).0;
    (v * GB_CART_W + w / 2) / w
}

fn gy(v: u32) -> u32 {
    let h = seated_box(Platform::Gb).1;
    (v * GB_CART_H + h / 2) / h
}

fn tmp_root() -> TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    for sub in [
        "Games",
        "Games/GBA",
        "Labels",
        "Saves",
        "States",
        "System",
        "Config",
    ] {
        std::fs::create_dir(d.path().join(sub)).expect("create content dir");
    }
    d
}

fn write_gb_rom(d: &TempDir, dir: &str, name: &str, cgb: u8) {
    let mut rom = vec![0u8; 0x150];
    rom[0x143] = cgb;
    let games = d.path().join("Games").join(dir);
    std::fs::create_dir_all(&games).expect("create games dir");
    std::fs::write(games.join(name), rom).expect("write rom");
}

fn write_rom(d: &TempDir, name: &str, title: &str) {
    let mut rom = vec![0u8; 0x100];
    rom[0xa0..0xa0 + title.len()].copy_from_slice(title.as_bytes());
    std::fs::write(d.path().join("Games/GBA").join(name), rom).expect("write rom");
}

fn write_label(d: &TempDir, name: &str, w: u32, h: u32, px: impl Fn(u32, u32) -> [u8; 3]) {
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        for x in 0..w {
            rgb.extend_from_slice(&px(x, y));
        }
    }
    std::fs::create_dir_all(d.path().join("Labels/GBA")).expect("create labels dir");
    let f = std::fs::File::create(d.path().join("Labels/GBA").join(name)).expect("create label");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(&rgb)
        .expect("png data");
}

fn pixel(face: &slot_ui::CartFace, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * face.w + x) * 4) as usize;
    [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
}

fn pixel_alpha(face: &slot_ui::CartFace, x: u32, y: u32) -> u8 {
    face.rgba[((y * face.w + x) * 4 + 3) as usize]
}

fn label_pixel(face: &slot_ui::CartFace, x: u32, y: u32) -> [u8; 3] {
    pixel(face, LABEL_X + x, LABEL_Y + y)
}

#[test]
fn a_malformed_label_falls_back_to_a_generated_one() {
    let d = tmp_root();
    write_rom(&d, "Broken.gba", "BROKEN");
    std::fs::create_dir_all(d.path().join("Labels/GBA")).unwrap();
    std::fs::write(d.path().join("Labels/GBA/Broken.png"), b"not a png").unwrap();
    let cart = &scan(d.path()).unwrap()[0];
    let face = cart_face(cart);
    assert_eq!((face.w, face.h), (CART_W, CART_H));
    assert!(face.rgba.iter().any(|b| *b != 0), "face is blank");
}

#[test]
fn label_colour_is_stable_across_calls() {
    assert_eq!(label_colour("POKEMON EMER"), label_colour("POKEMON EMER"));
    assert_ne!(label_colour("POKEMON EMER"), label_colour("ADVANCEWARS"));
}

#[test]
fn a_label_that_decodes_is_what_the_face_shows() {
    let d = tmp_root();
    write_rom(&d, "Labelled.gba", "LABELLED");
    write_label(&d, "Labelled.png", 64, 64, |_, _| [0xd0, 0x20, 0xa0]);
    let face = cart_face(&scan(d.path()).unwrap()[0]);
    assert_eq!((face.w, face.h), (CART_W, CART_H));
    for y in [0, LABEL_H / 2, LABEL_H - 1] {
        assert_eq!(
            label_pixel(&face, LABEL_W / 2, y),
            [0xd0, 0x20, 0xa0],
            "at centre,{y}"
        );
    }
    // A square cover is contained, so the wide label's sides stay empty.
    assert_ne!(label_pixel(&face, 0, LABEL_H / 2), [0xd0, 0x20, 0xa0]);
    assert_ne!(
        label_pixel(&face, LABEL_W - 1, LABEL_H / 2),
        [0xd0, 0x20, 0xa0]
    );
}

#[test]
fn a_label_of_the_wrong_aspect_is_letterboxed_not_cropped() {
    let d = tmp_root();
    write_rom(&d, "Tall.gba", "TALL");
    write_label(&d, "Tall.png", 160, 480, |_, y| match y / 160 {
        0 => [0xff, 0x00, 0x00],
        1 => [0xff, 0xff, 0xff],
        _ => [0x00, 0x00, 0xff],
    });
    let face = cart_face(&scan(d.path()).unwrap()[0]);
    let cx = LABEL_W / 2;
    assert_eq!(label_pixel(&face, cx, 0), [0xff, 0x00, 0x00]);
    assert_eq!(label_pixel(&face, cx, LABEL_H / 2), [0xff, 0xff, 0xff]);
    assert_eq!(
        label_pixel(&face, cx, LABEL_H - 1),
        [0x00, 0x00, 0xff]
    );
    assert_ne!(label_pixel(&face, 0, LABEL_H / 2), [0xff, 0xff, 0xff]);
}

#[test]
fn a_long_title_stays_inside_the_label() {
    for stem in [
        "Supercalifragilisticexpialidocious Anniversary Edition",
        "Super Mario Kart",
    ] {
        let d = tmp_root();
        std::fs::write(d.path().join(format!("Games/GBA/{stem}.gba")), [0u8; 8]).unwrap();
        let face = cart_face(&scan(d.path()).unwrap()[0]);
        let bg = label_colour(stem);
        let margin = 5;
        for y in 0..LABEL_H {
            for x in 0..LABEL_W {
                let edge =
                    x < margin || y < margin || x >= LABEL_W - margin || y >= LABEL_H - margin;
                if edge {
                    assert_eq!(
                        label_pixel(&face, x, y),
                        bg,
                        "{stem}: text spills into the border at {x},{y}"
                    );
                }
            }
        }
        assert!(
            (0..LABEL_H * LABEL_W).any(|i| label_pixel(&face, i % LABEL_W, i / LABEL_W) != bg),
            "{stem}: no text was drawn"
        );
    }
}

#[test]
fn the_cart_box_matches_the_reference_art() {
    let ratio = CART_W as f32 / CART_H as f32;
    assert!(
        (ratio - 1.782).abs() < 0.02,
        "aspect {ratio:.3}, the reference Advance art is being stretched"
    );
}

#[test]
fn the_label_is_wide_and_sits_low() {
    let (x0, y0, x1, y1) = label_panel(CART_W, CART_H);
    let (w, h) = (CART_W as f32, CART_H as f32);
    let side = x0 as f32 / w;
    let top = y0 as f32 / h;
    let bottom = 1.0 - y1 as f32 / h;
    let width = (x1 - x0) as f32 / w;

    assert!(
        width > 0.65,
        "the label is only {:.0}% of the cart wide, that reads as a panel not a label",
        width * 100.0
    );
    assert!(
        top > 0.18,
        "only {:.0}% of grip band above the label",
        top * 100.0
    );
    assert!(
        top > side * 1.2,
        "top band {top:.2} against side margin {side:.2}: that is a uniform border"
    );
    assert!(
        top > bottom * 1.6,
        "the label is not sitting low, top {top:.2} bottom {bottom:.2}"
    );
}

#[test]
fn the_body_is_narrower_than_its_grip_ears() {
    let m = silhouette(CART_W, CART_H);
    let solid = |x: u32, y: u32| m[(y * CART_W + x) as usize] > 128;
    let width_at = |y: u32| (0..CART_W).filter(|&x| solid(x, y)).count();

    let ears = width_at(CART_H / 12);
    let body = width_at(CART_H / 2);
    assert!(
        ears > body,
        "top spans {ears}px and the body {body}px: the grip ears are missing"
    );
    assert!(
        ears - body >= 3,
        "the ears stick out by only {}px, which will not read at all",
        ears - body
    );
    assert!(solid(CART_W / 2, CART_H / 2), "the middle is not solid");
    assert!(
        solid(CART_W / 2, CART_H - 2),
        "the bottom edge is not straight"
    );
}

#[test]
fn the_mega_drive_cart_uses_its_reference_shell_and_tall_label() {
    let cart = Cart {
        platform: Platform::Md,
        stem: "Sonic".into(),
        rom: PathBuf::from("Games/MD/Sonic.md"),
        label: None,
        code: String::new(),
        shell: None,
        title: "SONIC THE HEDGEHOG".into(),
    model: Default::default(),
    front: None,
};
    let face = cart_face(&cart);
    assert_eq!((face.w, face.h), (360, 244));
    assert_eq!(label_size(&cart), (256, 206));
}

#[test]
fn cart_faces_are_clipped_to_the_silhouette() {
    let d = tmp_root();
    write_rom(&d, "Emerald.gba", "EMERALD");
    let cart = &scan(d.path()).unwrap()[0];
    let f = cart_face(cart);
    let px = |x: u32, y: u32| f.rgba[((y * f.w + x) * 4 + 3) as usize];
    assert_eq!(
        px(2, 2),
        0,
        "the rounded corner is opaque, the mask was not applied"
    );
    assert!(px(f.w / 2, f.h / 2) > 250);
}

#[test]
fn a_supplied_label_is_clipped_to_the_silhouette_too() {
    let d = tmp_root();
    write_rom(&d, "Labelled.gba", "LABELLED");
    write_label(&d, "Labelled.png", LABEL_W, LABEL_H, |_, _| {
        [0x40, 0x80, 0xc0]
    });
    let f = cart_face(&scan(d.path()).unwrap()[0]);
    let px = |x: u32, y: u32| f.rgba[((y * f.w + x) * 4 + 3) as usize];
    assert_eq!(px(2, 2), 0, "the label escaped the rounded corner");
    assert!(px(f.w / 2, f.h / 2) > 250);
}

#[test]
fn the_label_is_the_filename_without_its_tags() {
    let cases = [
        (
            "Pokemon - Emerald Version (USA, Europe)",
            "Pokemon Emerald Version",
        ),
        (
            "Pokemon - LeafGreen Version (USA, Europe) (Rev 1)",
            "Pokemon LeafGreen Version",
        ),
        ("Shrek (USA) (Rev 6)", "Shrek"),
        ("Metroid Fusion", "Metroid Fusion"),
        ("Button Test", "Button Test"),
        ("Pokemon - Corrupt", "Pokemon Corrupt"),
        ("Some Game [!]", "Some Game"),
        ("Spaced   Out  (USA)", "Spaced Out"),
    ];
    for (stem, want) in cases {
        assert_eq!(clean_label(stem), want, "for {stem}");
    }
}

#[test]
fn an_unspaced_hyphen_survives() {
    assert_eq!(clean_label("Spider-Man 2 (USA)"), "Spider-Man 2");
    assert_eq!(
        clean_label("Wario Land 4 - Time Attack"),
        "Wario Land 4 Time Attack"
    );
}

#[test]
fn a_name_that_is_all_tags_falls_back_rather_than_going_blank() {
    assert_eq!(clean_label("(USA) (Rev 1)"), "(USA) (Rev 1)");
    assert_eq!(clean_label(""), "");
}

#[test]
fn the_header_title_no_longer_reaches_the_label() {
    let d = tmp_root();
    write_rom(
        &d,
        "Pokemon - Emerald Version (USA, Europe).gba",
        "POKEMON EMER",
    );
    let cart = &scan(d.path()).unwrap()[0];
    assert_eq!(label_text(cart), "Pokemon Emerald Version");
}

#[test]
fn two_regions_of_one_game_get_the_same_generated_colour() {
    let a = label_colour(&clean_label("Pokemon - Ruby Version (USA, Europe) (Rev 2)"));
    let b = label_colour(&clean_label("Pokemon - Ruby Version (Japan)"));
    assert_eq!(a, b, "the same game came out two colours");
}

#[test]
fn a_rom_with_no_header_title_is_labelled_from_its_stem() {
    let d = tmp_root();
    std::fs::write(d.path().join("Games/GBA/Homebrew Demo.gba"), [0u8; 8]).unwrap();
    let face = cart_face(&scan(d.path()).unwrap()[0]);
    let bg = label_colour("Homebrew Demo");
    assert!(
        (0..LABEL_H * LABEL_W).any(|i| label_pixel(&face, i % LABEL_W, i / LABEL_W) != bg),
        "an untitled rom got a blank label"
    );
}

#[test]
fn the_game_boy_pak_body_has_the_scanned_aspect() {
    let (sw, _) = seated_box(Platform::Gba);
    let body = |w: u32, h: u32| w as f64 / h as f64;
    assert!(
        (body(GB_CART_W, GB_CART_H) - 240.0 / 276.0).abs() < 0.01,
        "the pak body is {:.3}:1, not the reference 240x276 art",
        body(GB_CART_W, GB_CART_H)
    );
    let (gw, gh) = seated_box(Platform::Gb);
    assert_eq!(
        gw, sw,
        "both paks are 57 mm wide, so they seat at one width"
    );
    assert!((body(gw, gh) - 0.877).abs() < 0.01);
}

#[test]
fn the_game_boy_outline_has_parallel_sides_and_no_grip_ears() {
    for shell in [GbShell::Notched, GbShell::Rounded] {
        let gb = gb_silhouette(shell, GB_CART_W, GB_CART_H);
        let width_at = |y: u32| {
            (0..GB_CART_W)
                .filter(|&x| gb[(y * GB_CART_W + x) as usize] > 128)
                .count()
        };
        let (top, middle, bottom) = (
            width_at(GB_CART_H / 8),
            width_at(GB_CART_H / 2),
            width_at(GB_CART_H * 7 / 8),
        );
        assert_eq!(top, middle, "{shell:?}: wider at the top than the middle");
        assert_eq!(middle, bottom, "{shell:?}: tapers toward the bottom");

        let gba = silhouette(CART_W, CART_H);
        let gba_body = (0..CART_W)
            .filter(|&x| gba[((CART_H / 2) * CART_W + x) as usize] > 128)
            .count();
        let gba_body = (gba_body * GB_CART_W as usize + CART_W as usize / 2) / CART_W as usize;
        assert!(
            middle.abs_diff(gba_body) <= 2,
            "{shell:?}: the pak's body is {middle}px against the GBA body's {gba_body}px, \
             and both are 57 mm"
        );
    }
}

#[test]
fn the_colour_only_shell_loses_the_notch_and_rounds_the_corners() {
    let notched = gb_silhouette(GbShell::Notched, GB_CART_W, GB_CART_H);
    let rounded = gb_silhouette(GbShell::Rounded, GB_CART_W, GB_CART_H);
    fn covered(m: &[u8], y: u32) -> Vec<u32> {
        (0..GB_CART_W)
            .filter(|&x| m[(y * GB_CART_W + x) as usize] > 128)
            .collect()
    }
    let width_at = |m: &[u8], y: u32| covered(m, y).len();
    let left_at = |m: &[u8], y: u32| covered(m, y)[0];
    let right_at = |m: &[u8], y: u32| *covered(m, y).last().expect("an empty row");

    let y = 6;
    assert!(
        right_at(&rounded, y) > right_at(&notched, y) + 10,
        "the clear pak ends at {} against the notched {} at row {y}: the notch is not cut",
        right_at(&rounded, y),
        right_at(&notched, y)
    );
    assert!(
        left_at(&rounded, 0) > left_at(&notched, 0) + 10,
        "the top row starts at {} against the notched {}: the corners are not rounded",
        left_at(&rounded, 0),
        left_at(&notched, 0)
    );
    for y in 30..GB_CART_H {
        assert_eq!(
            width_at(&notched, y),
            width_at(&rounded, y),
            "the shells disagree at row {y}, which is below the shoulder"
        );
    }
}

#[test]
fn the_game_boy_label_well_is_near_square_and_sits_under_the_lettering_plate() {
    let (x0, y0, x1, y1) = gb_label_panel(GB_CART_W, GB_CART_H);
    let aspect = (x1 - x0) as f32 / (y1 - y0) as f32;
    assert!(
        (aspect - 1.135).abs() < 0.02,
        "the well is {aspect:.2}:1, which is not the 42x37 label's shape"
    );
    assert_eq!(x0, GB_CART_W - x1, "the well is not centred across the pak");
    let (above, below) = (y0, GB_CART_H - y1);
    assert!(
        above > below * 2,
        "{above}px above the label and {below}px below: the shoulder has lost its plate"
    );
}

#[test]
fn a_game_boy_cart_face_is_drawn_at_the_game_boy_size() {
    let d = tmp_root();
    write_gb_rom(&d, "GB", "Tetris.gb", 0x00);
    let face = cart_face(&scan(d.path()).unwrap()[0]);
    assert_eq!((face.w, face.h), (GB_CART_W, GB_CART_H));
    assert!(face.rgba.iter().any(|b| *b != 0), "face is blank");
}

#[test]
fn every_cartridge_is_centred_on_the_row_and_clears_both_the_plate_and_the_slot() {
    let lip = OUT_H as f32 - MOUTH_H;
    for (name, h, span) in [
        ("the GBA cart", CART_H, OUT_H as f32),
        ("the Game Boy pak", GB_CART_H, lip),
    ] {
        let (top, foot) = (rest_y(h as f32), foot_y(h as f32));
        assert_eq!(
            top + foot,
            span,
            "{name} stands at {top}..{foot}, which is not centred in {span}px"
        );
        assert!(
            top > PLATE_H,
            "{name}'s top edge at {top} is under the {PLATE_H}px HUD plate"
        );
        assert!(
            foot < lip,
            "{name}'s foot at {foot} has reached the lip at {lip}: it is standing in the slot"
        );
    }
}

#[test]
fn each_of_the_three_cgb_flags_gets_its_own_plastic() {
    let d = tmp_root();
    write_gb_rom(&d, "GB", "Tetris.gb", 0x00);
    write_gb_rom(&d, "GBC", "Colour Enhanced.gbc", 0x80);
    write_gb_rom(&d, "GBC", "Colour Only.gbc", 0xc0);
    let carts = scan(d.path()).unwrap();
    let by_stem = |stem: &str| {
        carts
            .iter()
            .find(|c| c.stem == stem)
            .unwrap_or_else(|| panic!("{stem} was not scanned"))
    };

    let grey = shell_for(by_stem("Tetris"));
    let black = shell_for(by_stem("Colour Enhanced"));
    let clear = shell_for(by_stem("Colour Only"));

    assert_eq!(grey.finish, Finish::Solid);
    assert_eq!(
        black.finish,
        Finish::Solid,
        "the 0x80 pak is black plastic, not clear"
    );
    assert_eq!(clear.finish, Finish::Translucent);

    let luma = |s: slot_ui::Shell| s.colour.iter().map(|c| *c as u32).sum::<u32>();
    assert!(
        luma(black) + 120 < luma(grey),
        "the black pak at {} is not darker than the grey one at {}",
        luma(black),
        luma(grey)
    );
    for (a, b) in [(grey, black), (black, clear), (grey, clear)] {
        assert_ne!(
            a.colour, b.colour,
            "two of the three plastics are one colour"
        );
    }
}

#[test]
fn a_game_boy_title_stays_inside_its_near_square_label() {
    for stem in [
        "Supercalifragilisticexpialidocious Anniversary Edition",
        "Wario Land 3",
        "Tetris",
    ] {
        let d = tmp_root();
        write_gb_rom(&d, "GB", &format!("{stem}.gb"), 0x00);
        let face = cart_face(&scan(d.path()).unwrap()[0]);
        let bg = label_colour(stem);
        let at = |x: u32, y: u32| pixel(&face, GB_LABEL_X + x, GB_LABEL_Y + y);
        let margin = 5;
        for y in 0..GB_LABEL_H {
            for x in 0..GB_LABEL_W {
                let edge = x < margin
                    || y < margin
                    || x >= GB_LABEL_W - margin
                    || y >= GB_LABEL_H - margin;
                if edge {
                    assert_eq!(
                        at(x, y),
                        bg,
                        "{stem}: text spills into the border at {x},{y}"
                    );
                }
            }
        }
        assert!(
            (0..GB_LABEL_H * GB_LABEL_W).any(|i| at(i % GB_LABEL_W, i / GB_LABEL_W) != bg),
            "{stem}: no text was drawn"
        );
    }
}

#[test]
fn the_cart_shadow_is_the_cart_in_black() {
    let s = slot_ui::cart_shadow();
    assert_eq!((s.w, s.h), (CART_W, CART_H));
    let mask = silhouette(CART_W, CART_H);
    for (px, cover) in s.rgba.chunks_exact(4).zip(&mask) {
        assert_eq!(&px[..3], &[0, 0, 0], "the shadow is not black");
        assert_eq!(px[3], *cover, "the shadow is not the cart's shape");
    }
    assert!(
        s.rgba.chunks_exact(4).any(|p| p[3] > 250),
        "the shadow is transparent everywhere, so it backs nothing"
    );
}

#[test]
fn each_game_boy_shell_gets_a_black_shadow_of_its_own_exact_outline() {
    for shell in [GbShell::Notched, GbShell::Rounded] {
        let s = slot_ui::gb_cart_shadow(shell);
        assert_eq!((s.w, s.h), (GB_CART_W, GB_CART_H));
        let own = gb_silhouette(shell, GB_CART_W, GB_CART_H);
        for (i, px) in s.rgba.chunks_exact(4).enumerate() {
            assert_eq!(&px[..3], &[0, 0, 0], "{shell:?}: the shadow is not black");
            assert_eq!(
                px[3], own[i],
                "{shell:?}: the shadow is not the shell's own shape at pixel {i}"
            );
        }
    }
    let notched = gb_silhouette(GbShell::Notched, GB_CART_W, GB_CART_H);
    let rounded = gb_silhouette(GbShell::Rounded, GB_CART_W, GB_CART_H);
    let bare: u32 = notched
        .iter()
        .zip(&rounded)
        .map(|(n, r)| u32::from(n.abs_diff(*r)))
        .sum();
    assert!(
        bare / 255 > 100,
        "the two shells now differ by {} px, so the shared backing was harmless after all",
        bare / 255
    );
}

#[test]
fn a_labelled_mini_cart_keeps_the_platform_shape_and_prints_its_label() {
    let gba = labelled_cart_face(Platform::Gba, None, DEFAULT_SHELL, "同步云存档");
    assert_eq!((gba.w, gba.h), (CART_W, CART_H));
    let gb = labelled_cart_face(
        Platform::Gb,
        Some(GbShell::Notched),
        DEFAULT_SHELL,
        "同步云存档",
    );
    assert_eq!((gb.w, gb.h), (GB_CART_W, GB_CART_H));
    let fc = labelled_cart_face(Platform::Fc, None, DEFAULT_SHELL, "同步云存档");
    assert_eq!((fc.w, fc.h), (FC_CART_W, FC_CART_H));

    // The label panel is opaque and coloured, and the printed words change it.
    let alpha = |face: &slot_ui::CartFace, x: u32, y: u32| {
        face.rgba[((y * face.w + x) * 4 + 3) as usize]
    };
    assert!(alpha(&gba, LABEL_X + 8, LABEL_Y + 8) > 0);
    let other = labelled_cart_face(
        Platform::Gba,
        None,
        DEFAULT_SHELL,
        "同步 GBAStation 电池存档同步",
    );
    assert_ne!(gba.rgba, other.rgba, "both titles printed the same label");

    // One shadow per shape: the wide GBA/FC cassette and the tall Game Boy.
    let wide = labelled_cart_shadow(Platform::Gba, None);
    assert_eq!((wide.w, wide.h), (CART_W, CART_H));
    let tall = labelled_cart_shadow(Platform::Gb, Some(GbShell::Rounded));
    assert_eq!((tall.w, tall.h), (GB_CART_W, GB_CART_H));
}
#[test]
fn the_famicom_cart_uses_the_reference_art() {
    assert_eq!(cart_box(Platform::Fc), (306, 254));
    assert_eq!(seated_box(Platform::Fc), (240, 199));
    assert_eq!(FC_CART_W, 360);
    assert_eq!(FC_CART_H, 299);
    assert_eq!(FC_BASE_W, 240);
    assert_eq!(FC_BASE_H, 199);

    let cart = Cart {
        platform: Platform::Fc,
        stem: "Contra".into(),
        rom: "Games/FC/Contra.nes".into(),
        label: None,
        title: String::new(),
        code: String::new(),
        shell: None,
    model: Default::default(),
    front: None,
};
    let face = cart_face(&cart);
    assert_eq!((face.w, face.h), (FC_CART_W, FC_CART_H));

    let nes = Cart {
        model: CartModel::Nes,
        ..cart.clone()
    };
    assert_eq!(label_size(&nes), (121, 200));
    let nes_face = cart_face(&nes);
    assert_eq!((nes_face.w, nes_face.h), (FC_CART_W, FC_CART_H));
    assert_ne!(face.rgba, nes_face.rgba, "NES art did not replace the Famicom face");

    let (left, top, right, bottom) = fc_label_panel(FC_CART_W, FC_CART_H);
    let (lw, lh) = (right - left, bottom - top);
    assert!(
        (lw as f32 / lh as f32 - 296.0 / 202.0).abs() < 0.03,
        "label is not the reference 296x202 box: {lw}x{lh}"
    );
    assert_eq!(left, 31);
    assert_eq!(top, 31);
    assert_eq!(right, 327);
    assert_eq!(bottom, 233);

    let pixel = |x: u32, y: u32| {
        let i = ((y * FC_CART_W + x) * 4) as usize;
        [
            face.rgba[i],
            face.rgba[i + 1],
            face.rgba[i + 2],
            face.rgba[i + 3],
        ]
    };
    // Orange plastic, not the old hand-drawn yellow tape.
    let plastic = pixel(20, 100);
    assert!(plastic[3] > 250, "the FC shell is not opaque");
    assert!(plastic[0] > 180 && plastic[1] > 100 && plastic[2] < 90);

    // A transparent cover shows label paper, not the key colour or shell.
    let (lw2, lh2) = label_size(&cart);
    let clear = cart_face_with(&cart, Some(vec![0u8; (lw2 * lh2 * 4) as usize]));
    let inside = {
        let i = (((top + 20) * FC_CART_W + left + 20) * 4) as usize;
        [
            clear.rgba[i],
            clear.rgba[i + 1],
            clear.rgba[i + 2],
            clear.rgba[i + 3],
        ]
    };
    assert_eq!(inside[3], 255, "the label left a hole: {inside:?}");
    assert!(inside[0] > 220 && inside[1] > 210 && inside[2] > 180);
}

#[test]
fn a_custom_cart_colour_tints_the_reference_shell() {
    let cart = |colour: Option<[u8; 3]>| Cart {
        platform: Platform::Gba,
        stem: "Colour Test".into(),
        rom: "Games/GBA/Colour Test.gba".into(),
        label: None,
        title: String::new(),
        code: String::new(),
        shell: colour.map(|colour| ShellChoice {
            outline: Outline::Auto,
            colour,
            finish: ShellFinish::Solid,
        }),
    model: Default::default(),
    front: None,
};
    let raw = cart_face(&cart(None));
    let red = cart_face(&cart(Some([0xc2, 0x33, 0x2e])));
    let pixel = |face: &slot_ui::CartFace, x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
    };
    let plain = pixel(&raw, 20, 100);
    let tinted = pixel(&red, 20, 100);
    assert_ne!(tinted, plain, "the configured colour did not reach the shell");
    assert!(
        tinted[0] > tinted[1] && tinted[0] > tinted[2],
        "the red shell lost its red: {tinted:?}"
    );
}

#[test]
fn a_custom_front_is_composited_over_the_cart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("front.png");
    let file = std::fs::File::create(&path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), FC_CART_W, FC_CART_H);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let rgba = vec![10u8, 200, 30, 255].repeat((FC_CART_W * FC_CART_H) as usize);
    writer.write_image_data(&rgba).unwrap();
    drop(writer);

    let cart = Cart {
        platform: Platform::Fc,
        stem: "Contra".into(),
        rom: "Games/FC/Contra.nes".into(),
        label: None,
        title: String::new(),
        code: String::new(),
        shell: None,
        model: Default::default(),
        front: Some(path),
    };
    let face = cart_face(&cart);
    assert_eq!((face.w, face.h), (FC_CART_W, FC_CART_H));
    assert_eq!(pixel(&face, FC_CART_W / 2, FC_CART_H / 2), [10, 200, 30]);
}

#[test]
fn a_receiver_preview_can_be_a_blank_platform_cart() {
    let cart = Cart {
        platform: Platform::Gba,
        stem: "Aladdin".into(),
        rom: PathBuf::new(),
        label: None,
        title: String::new(),
        code: String::new(),
        shell: None,
    model: Default::default(),
    front: None,
};
    let face = cart_face(&cart);
    assert!(face.w > 0 && face.h > 0, "blank preview has no face");
    assert!(
        face.rgba.chunks_exact(4).any(|px| px[3] > 0),
        "blank preview is fully transparent"
    );
}
