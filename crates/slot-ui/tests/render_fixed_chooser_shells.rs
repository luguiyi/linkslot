//! Mock the fixed chooser cartridge candidates. Opt-in: set `FIXED_SHELL_DIR`
//! to an output directory. Each row gets a cloud sticker and the two shipped
//! mod stickers on the same fixed shell.

use std::path::{Path, PathBuf};

use slot_store::Platform;
use slot_ui::{
    labelled_cart_face_with_sticker, sync_cart_face_with_sticker, CartFace, CartSticker, Finish,
    Shell, DEFAULT_SHELL, DMG_SHELL, SYNC_SHELL,
};

const CHARCOAL: Shell = DEFAULT_SHELL;
const PEARL: Shell = Shell {
    colour: [0xd9, 0xdb, 0xd8],
    finish: Finish::Translucent,
};
const FC_YELLOW: Shell = Shell {
    colour: [0xd8, 0xa8, 0x3e],
    finish: Finish::Solid,
};
const CRIMSON: Shell = Shell {
    colour: [0xc2, 0x33, 0x2e],
    finish: Finish::Translucent,
};

struct Option {
    slug: &'static str,
    platform: Platform,
    shell: Shell,
}

#[test]
fn render_fixed_chooser_shells() {
    let Ok(out) = std::env::var("FIXED_SHELL_DIR") else {
        return;
    };
    let Ok(out) = std::path::absolute(out) else {
        return;
    };
    std::fs::create_dir_all(&out).expect("create output dir");
    let mods = std::env::var("MOD_STICKER_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| Some(PathBuf::from("workdoc/examples")));
    let gba_station = mods
        .as_ref()
        .and_then(|dir| slot_ui::decode(&dir.join("gbastation-battery-sync/sticker.png")));
    let save_deck = mods
        .as_ref()
        .and_then(|dir| slot_ui::decode(&dir.join("savedeck-battery-bridge/sticker.png")));

    let options = [
        Option {
            slug: "charcoal",
            platform: Platform::Gba,
            shell: CHARCOAL,
        },
        Option {
            slug: "sync",
            platform: Platform::Gba,
            shell: SYNC_SHELL,
        },
        Option {
            slug: "pearl",
            platform: Platform::Gba,
            shell: PEARL,
        },
        Option {
            slug: "crimson",
            platform: Platform::Gba,
            shell: CRIMSON,
        },
        Option {
            slug: "dmg",
            platform: Platform::Gb,
            shell: DMG_SHELL,
        },
        Option {
            slug: "fc",
            platform: Platform::Fc,
            shell: FC_YELLOW,
        },
    ];

    for option in options {
        let cloud = if option.slug == "sync" {
            sync_cart_face_with_sticker(option.shell, "同步云存档", Some(CartSticker::Text))
        } else {
            labelled_cart_face_with_sticker(
                option.platform,
                None,
                option.shell,
                "同步云存档",
                Some(CartSticker::Text),
            )
        };
        write(&out, option.slug, "cloud", &cloud);

        if let Some((rgba, w, h)) = &gba_station {
            let face = if option.slug == "sync" {
                sync_cart_face_with_sticker(
                    option.shell,
                    "同步 GBAStation 电池存档同步",
                    Some(CartSticker::Art { rgba, w: *w, h: *h }),
                )
            } else {
                labelled_cart_face_with_sticker(
                    option.platform,
                    None,
                    option.shell,
                    "同步 GBAStation 电池存档同步",
                    Some(CartSticker::Art { rgba, w: *w, h: *h }),
                )
            };
            write(&out, option.slug, "gba", &face);
        }
        if let Some((rgba, w, h)) = &save_deck {
            let face = if option.slug == "sync" {
                sync_cart_face_with_sticker(
                    option.shell,
                    "同步 SaveDeck 电池存档桥",
                    Some(CartSticker::Art { rgba, w: *w, h: *h }),
                )
            } else {
                labelled_cart_face_with_sticker(
                    option.platform,
                    None,
                    option.shell,
                    "同步 SaveDeck 电池存档桥",
                    Some(CartSticker::Art { rgba, w: *w, h: *h }),
                )
            };
            write(&out, option.slug, "savedeck", &face);
        }
    }
}

fn write(dir: &Path, skin: &str, card: &str, face: &CartFace) {
    let path = dir.join(format!("{skin}-{card}.png"));
    let file = std::fs::File::create(&path).expect("create png");
    let mut encoder = png::Encoder::new(file, face.w, face.h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("png header")
        .write_image_data(&face.rgba)
        .expect("png data");
}
