//! Per-ROM cartridge colours that the Web game detail page and the initial
//! library scan share.
//!
//! Stored separately from `cart_shell.txt`: that older table is a hand-written
//! shell style (outline + colour + finish), while this file is just the colour
//! the card art should be tinted with.  Keys are platform-qualified so a GBA
//! and a GB game with the same stem do not share a colour.

use std::collections::HashMap;
use std::io;
use std::path::Path;

use crate::cart_shell::key as stem_key;
use crate::{Cart, Outline, Platform, ShellChoice, ShellFinish};

pub const CART_COLOR_FILE: &str = "linkSlot/Config/cart_color.txt";

/// The platform-qualified key used in `linkSlot/Config/cart_color.txt`.
pub fn key(platform: Platform, stem: &str) -> String {
    format!("{}/{}", platform.dir_name(), stem_key(stem))
}

/// Parse `#rrggbb` or `rrggbb`.
pub fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let hex = value.trim().trim_start_matches('#');
    if hex.len() != 6 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([0, 2, 4].map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0)))
}

pub fn to_hex(colour: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", colour[0], colour[1], colour[2])
}

fn parse(text: &str) -> HashMap<String, [u8; 3]> {
    crate::ini::parse(text)
        .into_iter()
        .filter_map(|(stem, value)| Some((stem, parse_hex(&value)?)))
        .collect()
}

pub fn read(root: &Path) -> HashMap<String, [u8; 3]> {
    std::fs::read_to_string(root.join(CART_COLOR_FILE))
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// A stable, well-spaced colour derived from the ROM identity.  The colour is
/// persisted on first scan, so it changes only when the user changes it; this
/// function is deterministic so a lost config is restored to the same palette
/// rather than a different random colour on every boot.
pub fn initial(platform: Platform, stem: &str) -> [u8; 3] {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in platform.dir_name().bytes().chain(stem.bytes()) {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let hue = (hash % 360) as f32;
    let saturation = 0.48 + ((hash >> 16) % 22) as f32 / 100.0;
    let value = 0.74 + ((hash >> 32) % 18) as f32 / 100.0;
    hsv(hue, saturation, value)
}

fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0).round() as u8,
        ((g + m) * 255.0).round() as u8,
        ((b + m) * 255.0).round() as u8,
    ]
}

/// Give every cart with no colour an initial one and write the new entries.
/// Returns whether the config file was changed.  Existing `cart_shell.txt`
/// choices win and are never overwritten here.
pub fn assign_missing(root: &Path, carts: &mut [Cart]) -> io::Result<bool> {
    let path = root.join(CART_COLOR_FILE);
    let mut text = std::fs::read_to_string(&path).unwrap_or_default();
    let mut changed = false;
    for cart in carts.iter_mut() {
        if cart.shell.is_some() {
            continue;
        }
        let colour = initial(cart.platform, &cart.stem);
        text = crate::ini::set(&text, &key(cart.platform, &cart.stem), Some(&to_hex(colour)))
            .map_err(|e| io::Error::new(e.kind(), format!("{CART_COLOR_FILE}: {e}")))?;
        cart.shell = Some(ShellChoice {
            outline: Outline::Auto,
            colour,
            finish: ShellFinish::Solid,
        });
        changed = true;
    }
    if changed {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        crate::atomic_write(&path, text.as_bytes())?;
    }
    Ok(changed)
}

/// Persist one colour, leaving all other entries alone.
pub fn write(root: &Path, platform: Platform, stem: &str, colour: [u8; 3]) -> io::Result<()> {
    crate::ini::write(
        root,
        CART_COLOR_FILE,
        &key(platform, stem),
        &to_hex(colour),
    )
}
