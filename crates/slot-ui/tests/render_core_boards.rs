//! Renders the core picker board once per platform, so the per-platform PCBs,
//! sockets and chips can be compared by eye. Opt-in: does nothing unless
//! `CORE_BOARD_PNG` names a file.

use slot_store::{Cart, Core, Platform};
use slot_ui::{
    board_face, board_size, board_units, chip_face_for, network_chip_face, network_spot, single_core_spot, socket_face, CartFace,
    CHIP_U, CHIP_V, SOCKET_U, SOCKET_V, TURN_PAD,
};

const GAP: u32 = 6;

fn cart(platform: Platform) -> Cart {
    Cart {
        platform,
        stem: "POKEMON EMER".into(),
        rom: format!("Games/{}/POKEMON EMER.gba", platform.dir_name()).into(),
        label: None,
        code: "BPEE".into(),
        shell: None,
        title: "POKEMON EMER".into(),
    model: Default::default(),
    front: None,
}
}

fn on_board(platform: Platform, u: f32, v: f32) -> (u32, u32) {
    let (bw, bh) = board_size(platform);
    let (uw, uh) = board_units(platform);
    (
        (u * bw as f32 / uw).round() as u32,
        (v * bh as f32 / uh).round() as u32,
    )
}

/// Paste `face` into `frame` at (left, top), scaled by `k`.
fn paste(frame: &mut [u8], sheet_w: u32, face: &CartFace, left: u32, top: u32, k: f32) {
    for y in 0..(face.h as f32 * k).round() as u32 {
        for x in 0..(face.w as f32 * k).round() as u32 {
            let s = ((y / k as u32).min(face.h - 1) * face.w + x / k as u32) as usize * 4;
            let a = face.rgba[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = (((top + y) * sheet_w + left + x) * 3) as usize;
            for c in 0..3 {
                frame[d + c] =
                    ((face.rgba[s + c] as u32 * a + frame[d + c] as u32 * (255 - a) + 127) / 255)
                        as u8;
            }
        }
    }
}

#[test]
fn render_core_boards() {
    let Ok(out) = std::env::var("CORE_BOARD_PNG") else {
        return;
    };
    let rows: [(Platform, &[Core]); 4] = [
        (Platform::Gba, &[Core::Mgba, Core::Gpsp]),
        (Platform::Gb, &[Core::Mgba]),
        (Platform::Gbc, &[Core::Mgba]),
        (Platform::Fc, &[Core::Nes]),
    ];

    let sheet_w = rows.iter().map(|(p, _)| board_size(*p).0).max().unwrap_or(1);
    let sheet_h = rows
        .iter()
        .map(|(p, _)| board_size(*p).1 + GAP)
        .sum::<u32>();
    let mut sheet = vec![0x14u8; (sheet_w * sheet_h * 3) as usize];

    let mut top = 0;
    for (platform, options) in rows {
        let (bw, bh) = board_size(platform);
        let board = board_face(&cart(platform));
        assert_eq!((board.w, board.h), (bw, bh));
        let left = (sheet_w - bw) / 2;
        paste(&mut sheet, sheet_w, &board, left, top, 1.0);

        let net = network_chip_face(Some("云存档"));
        let (u, v) = network_spot(platform);
        let (x, y) = on_board(platform, u, v);
        paste(&mut sheet, sheet_w, &net, left + x, top + y, 1.0);

        let single = options.len() == 1;
        let spot = if single {
            Some(single_core_spot(platform))
        } else {
            None
        };
        for (position, core) in options.iter().enumerate() {
            let (u, v) = match spot {
                Some((u, v, _, _)) => (u, v),
                None => (SOCKET_U[position], SOCKET_V),
            };
            let (x, y) = on_board(platform, u, v);
            let socket = socket_face(*core);
            paste(&mut sheet, sheet_w, &socket, left + x, top + y, 1.0);
        }

        // The chip sits in the first socket, the way the picker opens it.
        let seated = options[0];
        let (u, v) = match spot {
            Some((_, _, u, v)) => (u, v),
            None => (CHIP_U[0], CHIP_V),
        };
        let (x, y) = on_board(platform, u, v);
        let chip = chip_face_for(platform, Some(seated));
        paste(
            &mut sheet,
            sheet_w,
            &chip,
            left + x.saturating_sub(TURN_PAD),
            top + y.saturating_sub(TURN_PAD),
            1.0,
        );
        top += bh + GAP;
    }

    let f = std::fs::File::create(&out).expect("create png");
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), sheet_w, sheet_h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()
        .expect("png header")
        .write_image_data(&sheet)
        .expect("png data");
    println!("wrote {out}");
}
