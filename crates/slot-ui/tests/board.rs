use slot_store::{Cart, Core, Outline, Platform, ShellChoice, ShellFinish};
use slot_ui::{
    board_at, board_face, chip_face, chip_shadow_face, gba_shell_for, lid_at, lift_of, on_board,
    padded, rom_marking, rom_marking_face, shelf_cart, single_core_spot, slide_of, socket_face,
    board_from_for, board_size, board_units, core_chip_scale, lid_from_for, CartFace, Placed, BOARD_H, BOARD_W, CART_H, CAVITIES, CHIP_H, CHIP_W, DEFAULT_SHELL, LID_TURN,
    ROM_H, ROM_W, SHADOW_H, SHADOW_W, SLIDE_SHARE, SLIDE_UP, SOCKET_H, SOCKET_W, TURN_PAD,
};

/// The board's own units, the space the socket and the cavity are written in.
const UNITS_W: f32 = 240.0;
const UNITS_H: f32 = 135.0;

fn cart(stem: &str, code: &str) -> Cart {
    Cart {
        platform: Platform::Gba,
        stem: stem.into(),
        rom: format!("Games/GBA/{stem}.gba").into(),
        label: None,
        code: code.into(),
        shell: None,
        title: stem.to_uppercase(),
    model: Default::default(),
    front: None,
}
}

fn cart_of(platform: Platform, stem: &str) -> Cart {
    Cart {
        platform,
        stem: stem.into(),
        rom: format!("Games/{}/{stem}.rom", platform.dir_name()).into(),
        label: None,
        code: String::new(),
        shell: None,
        title: stem.to_uppercase(),
    model: Default::default(),
    front: None,
}
}

fn rgb(face: &CartFace, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * face.w + x) * 4) as usize;
    [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    (0..3).all(|k| (a[k] as i32 - b[k] as i32).abs() <= 3)
}

const EMERALD: &str = "Pokemon - Emerald Version (USA, Europe)";

#[test]
fn the_marking_is_the_title_stacked() {
    assert_eq!(rom_marking(EMERALD), ["POKEMON", "EMERALD", "VERSION"]);
}

#[test]
fn a_long_title_folds_its_tail_into_the_third_line() {
    assert_eq!(
        rom_marking("Advance Wars 2 - Black Hole Rising"),
        ["ADVANCE", "WARS 2", "BLACK HOLE RISING"]
    );
}

#[test]
fn bracketed_tags_never_reach_the_chip() {
    assert_eq!(
        rom_marking("Pokemon - LeafGreen Version (USA, Europe) (Rev 1)"),
        ["POKEMON", "LEAFGREEN", "VERSION"]
    );
    assert_eq!(rom_marking("Metroid Fusion"), ["METROID", "FUSION"]);
}

#[test]
fn the_marking_is_drawn_and_stays_on_the_chip() {
    for stem in [
        EMERALD,
        "Metroid Fusion",
        "Advance Wars 2 - Black Hole Rising",
        "Pokemon - LeafGreen Version (USA, Europe) (Rev 1)",
    ] {
        let face = rom_marking_face(stem);
        assert_eq!((face.w, face.h), (ROM_W, ROM_H));
        let alpha = |x: u32, y: u32| face.rgba[((y * face.w + x) * 4 + 3) as usize];
        assert!(
            (0..face.h).any(|y| (0..face.w).any(|x| alpha(x, y) > 128)),
            "{stem} left no marking"
        );
        for y in 0..face.h {
            assert_eq!(alpha(0, y), 0, "{stem} ran off the left of the chip");
            assert_eq!(
                alpha(face.w - 1, y),
                0,
                "{stem} ran off the right of the chip"
            );
        }
    }
}

#[test]
fn the_cell_carries_no_print() {
    let face = board_face(&cart(EMERALD, "BPEE"));
    let dark = (50..69)
        .flat_map(|y| (307..326).map(move |x| (x, y)))
        .filter(|&(x, y)| (x as i32 - 316).pow(2) + (y as i32 - 59).pow(2) <= 81)
        .filter(|&(x, y)| rgb(&face, x, y)[0] < 150)
        .count();
    assert_eq!(dark, 0, "the cell still has {dark} dark pixels of print");
}

#[test]
fn the_board_is_the_size_it_is_shown_at() {
    let face = board_face(&cart(EMERALD, "BPEE"));
    assert_eq!((face.w, face.h), (BOARD_W, BOARD_H));
    assert_eq!((BOARD_W, BOARD_H), (372, 209));
}

#[test]
fn the_back_shell_is_the_carts_own_plastic() {
    let emerald = board_face(&cart(EMERALD, "BPEE"));
    assert!(
        near(rgb(&emerald, 14, 70), gba_shell_for("BPEE").colour),
        "Emerald's wall is {:?}",
        rgb(&emerald, 14, 70)
    );
    let unknown = board_face(&cart("Homebrew", ""));
    assert!(
        near(rgb(&unknown, 14, 70), DEFAULT_SHELL.colour),
        "an unknown cart's wall is {:?}",
        rgb(&unknown, 14, 70)
    );
}

#[test]
fn the_board_itself_is_drawn() {
    let face = board_face(&cart(EMERALD, "BPEE"));
    assert!(
        near(rgb(&face, 310, 108), [0x3a, 0x9a, 0x3c]),
        "the board is {:?}",
        rgb(&face, 310, 108)
    );
}

fn alpha(face: &CartFace, x: u32, y: u32) -> u8 {
    face.rgba[((y * face.w + x) * 4 + 3) as usize]
}

#[test]
fn the_open_cart_starts_as_the_shelf_cart_and_lands_where_the_mockup_has_it() {
    assert_eq!(
        shelf_cart(),
        Placed {
            x: 180.0,
            y: 139.0,
            w: 360.0,
            h: 202.0
        }
    );
    assert_eq!(board_at(0.0), shelf_cart());
    assert_eq!(
        board_at(1.0),
        Placed {
            x: 174.0,
            y: 150.0,
            w: 372.0,
            h: 209.0
        }
    );
    assert_eq!(board_at(1.7), board_at(1.0), "an overshoot kept growing");
}

#[test]
fn the_lid_leaves_level_and_rests_turned() {
    assert_eq!(lid_at(0.0), (shelf_cart(), 0.0));
    let (rest, turn) = lid_at(1.0);
    assert_eq!(
        rest,
        Placed {
            x: 288.0,
            y: 30.0,
            w: 144.0,
            h: 81.0
        }
    );
    assert_eq!(turn, LID_TURN);
    assert!(
        (LID_TURN.to_degrees() + 5.0).abs() < 1e-4,
        "{}",
        LID_TURN.to_degrees()
    );
}

#[test]
fn the_beats_split_one_progress() {
    assert_eq!((slide_of(0.0), lift_of(0.0)), (0.0, 0.0));
    assert_eq!((slide_of(SLIDE_SHARE), lift_of(SLIDE_SHARE)), (1.0, 0.0));
    assert_eq!((slide_of(1.0), lift_of(1.0)), (1.0, 1.0));
    assert!((slide_of(SLIDE_SHARE / 2.0) - 0.5).abs() < 1e-5);
}

#[test]
fn the_back_half_stays_on_the_shelf_while_the_front_slides() {
    for p in [0.0, 0.1, 0.25, SLIDE_SHARE] {
        assert_eq!(board_at(p), shelf_cart(), "the back moved at progress {p}");
    }
}

#[test]
fn the_front_slides_up_a_third_before_it_lifts() {
    assert_eq!(SLIDE_UP, CART_H as f32 / 3.0);
    let shelf = shelf_cart();
    let (slid, turn) = lid_at(SLIDE_SHARE);
    assert_eq!(
        slid,
        Placed {
            y: shelf.y - SLIDE_UP,
            ..shelf
        }
    );
    assert_eq!(turn, 0.0);
    let (half, turn) = lid_at(SLIDE_SHARE / 2.0);
    assert!(half.y < shelf.y && half.y > slid.y && turn == 0.0 && half.w == shelf.w);
}

#[test]
fn the_lift_starts_where_the_slide_ends() {
    let (slid, _) = lid_at(SLIDE_SHARE);
    let (next, _) = lid_at(SLIDE_SHARE + 0.001);
    assert!((next.y - slid.y).abs() < 0.5 && (next.w - slid.w).abs() < 0.5);
}

#[test]
fn board_units_land_on_the_panel_at_one_and_a_half_times() {
    let board = board_at(1.0);
    assert_eq!(on_board(board, 0.0, 0.0), (174.0, 150.0));
    assert_eq!(on_board(board, 240.0, 135.0), (546.0, 359.0));
}

#[test]
fn a_socket_names_its_core_at_half_strength() {
    for core in Core::ALL {
        let face = socket_face(core);
        assert_eq!((face.w, face.h), (SOCKET_W, SOCKET_H));
        let strongest = (12..34)
            .flat_map(|y| (16..48).map(move |x| (x, y)))
            .map(|(x, y)| alpha(&face, x, y))
            .max()
            .unwrap();
        assert!(
            (60..=150).contains(&strongest),
            "{core:?}'s name is at alpha {strongest}"
        );
    }
}

#[test]
fn a_partly_covered_socket_edge_pixel_keeps_the_outline_inks_brightness() {
    let face = socket_face(Core::Mgba);
    let (r, a) = (6..(face.h - 6))
        .find_map(|y| {
            let i = ((y * face.w) * 4) as usize;
            let a = face.rgba[i + 3];
            (a > 0 && a < 255).then(|| (face.rgba[i], a))
        })
        .expect("no partly covered pixel on the socket's left edge");
    assert!(
        r > 200,
        "the outline ink darkened at partial coverage: R={r} at alpha={a}"
    );
}

#[test]
fn the_chip_is_padded_clear_on_every_side() {
    for core in [None, Some(Core::Mgba), Some(Core::Gpsp)] {
        let face = chip_face(core);
        assert_eq!(
            (face.w, face.h),
            (CHIP_W + 2 * TURN_PAD, CHIP_H + 2 * TURN_PAD)
        );
        for y in 0..face.h {
            for x in 0..face.w {
                let ring = x < TURN_PAD
                    || y < TURN_PAD
                    || x >= face.w - TURN_PAD
                    || y >= face.h - TURN_PAD;
                if ring {
                    assert_eq!(alpha(&face, x, y), 0, "{core:?} is not clear at {x},{y}");
                }
            }
        }
    }
}

#[test]
fn a_seated_chip_wears_its_name_and_a_flying_one_is_blank() {
    let light = |face: &CartFace| {
        face.rgba
            .chunks_exact(4)
            .filter(|p| p[0] > 200 && p[1] > 200 && p[2] > 200 && p[3] > 200)
            .count()
    };
    assert!(
        light(&chip_face(Some(Core::Mgba))) > 20,
        "mGBA left no name"
    );
    assert_eq!(light(&chip_face(None)), 0, "the blank chip says something");
    assert_ne!(
        chip_face(Some(Core::Mgba)).rgba,
        chip_face(Some(Core::Gpsp)).rgba,
        "both cores wear the same mark"
    );
}

#[test]
fn padding_moves_nothing() {
    let face = chip_face(Some(Core::Gpsp));
    let more = padded(&face, 3);
    assert_eq!((more.w, more.h), (face.w + 6, face.h + 6));
    for (x, y) in [(0, 0), (10, 7), (face.w - 1, face.h - 1)] {
        let a = ((y * face.w + x) * 4) as usize;
        let b = (((y + 3) * more.w + x + 3) * 4) as usize;
        assert_eq!(face.rgba[a..a + 4], more.rgba[b..b + 4]);
    }
}

#[test]
fn the_shadow_is_darkest_in_the_middle_and_gone_at_the_corners() {
    let s = chip_shadow_face();
    assert_eq!((s.w, s.h), (SHADOW_W, SHADOW_H));
    assert!(alpha(&s, SHADOW_W / 2, SHADOW_H / 2) > alpha(&s, SHADOW_W / 8, SHADOW_H / 2));
    assert_eq!(alpha(&s, 0, 0), 0);
}

#[test]
fn the_seated_chips_name_is_antialiased_against_the_body() {
    let red = |face: &CartFace, x: u32, y: u32| face.rgba[((y * face.w + x) * 4) as usize];
    for core in [Core::Mgba, Core::Gpsp] {
        let named = chip_face(Some(core));
        let blank = chip_face(None);
        let soft = (0..named.h)
            .flat_map(|y| (0..named.w).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                alpha(&named, x, y) == 255
                    && red(&named, x, y) != red(&blank, x, y)
                    && (0x40..0xc0).contains(&red(&named, x, y))
            })
            .count();
        assert!(
            soft > 5,
            "{core:?}'s name has only {soft} antialiased pixels"
        );
    }
}

/// The brightness of a pixel: the cavity a board draws is a near black recess
/// over green, the board around it is the green itself.
fn brightness(face: &CartFace, x: u32, y: u32) -> u32 {
    let i = ((y * face.w + x) * 4) as usize;
    face.rgba[i] as u32 + face.rgba[i + 1] as u32 + face.rgba[i + 2] as u32
}

fn at_units(face: &CartFace, platform: Platform, u: f32, v: f32) -> u32 {
    let (bw, bh) = board_size(platform);
    let (uw, uh) = board_units(platform);
    let x = (u * bw as f32 / uw).round() as u32;
    let y = (v * bh as f32 / uh).round() as u32;
    brightness(face, x.min(face.w - 1), y.min(face.h - 1))
}

#[test]
fn the_cavity_each_single_core_board_draws_is_where_the_table_says() {
    for (platform, (left, top, right, bottom)) in CAVITIES {
        let face = board_face(&cart_of(platform, "Tetris"));
        let inset = 3.0;
        for (u, v) in [
            (left + inset, top + inset),
            (right - inset, top + inset),
            (left + inset, bottom - inset),
            (right - inset, bottom - inset),
        ] {
            assert!(
                at_units(&face, platform, u, v) < 160,
                "{platform:?}'s cavity is not dark at {u},{v}"
            );
        }
        // Just outside the cavity is the board's green, or the frame around it.
        for (u, v) in [(left - inset, top - inset), (right + inset, bottom + inset)] {
            assert!(
                at_units(&face, platform, u, v) > 160,
                "{platform:?}'s cavity runs past the board at {u},{v}"
            );
        }
    }
}

#[test]
fn the_single_core_socket_and_chip_sit_inside_the_cavity() {
    for (platform, (left, top, right, bottom)) in CAVITIES {
        let (bw, bh) = board_size(platform);
        let (uw, uh) = board_units(platform);
        let zoom = bw as f32 / BOARD_W as f32;
        let core = core_chip_scale(platform);
        let socket_w = SOCKET_W as f32 * zoom * uw / bw as f32 * core;
        let socket_h = SOCKET_H as f32 * zoom * uh / bh as f32 * core;
        let chip_w = CHIP_W as f32 * zoom * uw / bw as f32 * core;
        let chip_h = CHIP_H as f32 * zoom * uh / bh as f32 * core;
        let (socket_u, socket_v, chip_u, chip_v) = single_core_spot(platform);
        assert!(
            socket_u >= left && socket_u + socket_w <= right,
            "{platform:?}'s socket runs out of its cavity"
        );
        assert!(
            socket_v >= top && socket_v + socket_h <= bottom,
            "{platform:?}'s socket runs out of its cavity"
        );
        assert!(
            chip_u >= left && chip_u + chip_w <= right,
            "{platform:?}'s chip runs out of its cavity"
        );
        assert!(
            chip_v >= top && chip_v + chip_h <= bottom,
            "{platform:?}'s chip runs out of its cavity"
        );
        // The chip is the socket's middle, the way the GBA board's two chips
        // sit in their sockets.
        assert!(
            (socket_u + socket_w / 2.0 - (chip_u + chip_w / 2.0)).abs() < 0.5,
            "{platform:?}'s chip is off the socket's middle"
        );
        assert!(
            (socket_v + socket_h / 2.0 - (chip_v + chip_h / 2.0)).abs() < 0.5,
            "{platform:?}'s chip is off the socket's middle"
        );
    }
}

/// SFC draws the Famicom board art, so its core socket and chip must use the
/// same cavity. A missing table row used to fall through to (0, 0, 0, 0) and
/// put the core in the board's top-left corner.
#[test]
fn the_super_famicom_core_sits_where_the_famicom_core_does() {
    let sfc = single_core_spot(Platform::Sfc);
    let fc = single_core_spot(Platform::Fc);
    assert_eq!(sfc, fc, "SFC's core spot drifted from the Famicom board");
    assert!(sfc.0 > 0.0 && sfc.1 > 0.0, "SFC fell back to the origin");
}

#[test]
fn the_gb_board_and_lid_keep_the_portrait_aspect_while_opening() {
    let shelf = Placed {
        x: 180.0,
        y: 139.0,
        w: 240.0,
        h: 276.0,
    };
    let target = 300.0 / 342.0;
    for platform in [Platform::Gb, Platform::Gbc] {
        for t in [0.0, 0.2, 0.4, 0.6, 0.8, 1.0] {
            let board = board_from_for(platform, shelf, t);
            let aspect = board.w / board.h;
            assert!(
                (aspect - target).abs() < 0.01,
                "{platform:?} board aspect at {t} is {aspect}, wanted {target}"
            );
        }
        let (lid, _) = lid_from_for(platform, shelf, 1.0);
        let lid_aspect = lid.w / lid.h;
        assert!(
            (lid_aspect - 240.0 / 276.0).abs() < 0.01,
            "{platform:?} lid aspect at rest is {lid_aspect}"
        );
    }
}


#[test]
fn the_mega_drive_board_and_lid_keep_the_cassette_aspect() {
    let shelf = Placed {
        x: 180.0,
        y: 154.0,
        w: 360.0,
        h: 244.0,
    };
    let target = 372.0 / 252.0;
    for t in [0.0, 0.2, 0.4, 0.6, 0.8, 1.0] {
        let board = board_from_for(Platform::Md, shelf, t);
        let aspect = board.w / board.h;
        assert!(
            (aspect - target).abs() < 0.01,
            "MD board aspect at {t} is {aspect}, wanted {target}"
        );
    }
    let (lid, _) = lid_from_for(Platform::Md, shelf, 1.0);
    let lid_aspect = lid.w / lid.h;
    assert!(
        (lid_aspect - 360.0 / 244.0).abs() < 0.01,
        "MD lid aspect at rest is {lid_aspect}"
    );
    let (socket_u, socket_v, _, _) = single_core_spot(Platform::Md);
    assert!(
        socket_u > 0.0 && socket_v > 0.0,
        "MD core fell back to the origin"
    );
}

/// The MD board is not the Famicom's any more: its cavity has to match the
/// new `board_md.svg`, not the FC fallback.
#[test]
fn the_mega_drive_core_uses_its_own_board_cavity() {
    let (socket_u, socket_v, _, _) = single_core_spot(Platform::Md);
    let (fc_u, fc_v, _, _) = single_core_spot(Platform::Fc);
    assert!(
        (socket_u, socket_v) != (fc_u, fc_v),
        "MD is still borrowing the Famicom cavity"
    );
}

#[test]
fn the_fc_board_and_lid_keep_the_wide_aspect_while_opening() {
    let shelf = Placed {
        x: 180.0,
        y: 154.0,
        w: 360.0,
        h: 299.0,
    };
    let target = 372.0 / 310.0;
    for t in [0.0, 0.2, 0.4, 0.6, 0.8, 1.0] {
        let board = board_from_for(Platform::Fc, shelf, t);
        let aspect = board.w / board.h;
        assert!(
            (aspect - target).abs() < 0.01,
            "FC board aspect at {t} is {aspect}, wanted {target}"
        );
    }
    let (lid, _) = lid_from_for(Platform::Fc, shelf, 1.0);
    let lid_aspect = lid.w / lid.h;
    assert!(
        (lid_aspect - 144.0 / 120.0).abs() < 0.01,
        "FC lid aspect at rest is {lid_aspect}"
    );
}


#[test]
fn opened_gb_gbc_and_fc_shells_follow_the_cart_shell() {
    for platform in [Platform::Gb, Platform::Gbc, Platform::Fc] {
        let mut cart = cart_of(platform, "Shell Test");
        cart.shell = Some(ShellChoice {
            outline: Outline::Auto,
            colour: [0xc2, 0x33, 0x2e],
            finish: ShellFinish::Solid,
        });
        let face = board_face(&cart);
        let at = ((100 * face.w + 4) * 4) as usize;
        let [r, g, b, a] = [
            face.rgba[at],
            face.rgba[at + 1],
            face.rgba[at + 2],
            face.rgba[at + 3],
        ];
        assert!(a > 0, "{platform:?} shell pixel is transparent");
        assert!(
            r > g.saturating_add(10) && r > b.saturating_add(10),
            "{platform:?} board shell did not use the red cartridge shell: {r},{g},{b}"
        );
    }
}

#[test]
fn sfc_has_two_core_sockets_and_other_machines_have_none() {
    let first = slot_ui::multi_core_slot(Platform::Sfc, 0).unwrap();
    let second = slot_ui::multi_core_slot(Platform::Sfc, 1).unwrap();
    assert!(
        (second.0 - first.0 - slot_ui::SFC_SLOT_PITCH).abs() < 1e-3,
        "SFC 的第二个插槽间距不对"
    );
    assert!(slot_ui::multi_core_slot(Platform::Sfc, 2).is_none());
    assert!(
        slot_ui::multi_core_slot(Platform::Fc, 0).is_none(),
        "FC 仍是单插槽"
    );

    let gba0 = slot_ui::multi_core_slot(Platform::Gba, 0).unwrap();
    let gba1 = slot_ui::multi_core_slot(Platform::Gba, 1).unwrap();
    assert!((gba1.0 - gba0.0 - 72.0).abs() < 1e-3, "GBA 双插槽间距变了");
    assert!(slot_ui::multi_core_slot(Platform::Gba, 2).is_none());

    let (u0, _) = slot_ui::chip_along(Platform::Sfc, 0.0).unwrap();
    let (u1, _) = slot_ui::chip_along(Platform::Sfc, 1.0).unwrap();
    assert!((u1 - u0 - slot_ui::SFC_SLOT_PITCH).abs() < 1e-3);
}