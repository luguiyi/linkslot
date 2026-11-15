use slot_ui::{playtime_plate, MOUTH_H, OUT_H, OUT_W, STATUS_H};

#[test]
fn the_plate_sits_below_the_selected_cartridge() {
    let cart = (240.0, 134.0, 240.0, 141.0);
    let (x, y, w, h) = playtime_plate(cart, (200, 20));
    assert_eq!((w, h), (212.0, 32.0));
    assert_eq!(x, 254.0);
    assert!(
        y >= cart.1 + cart.3 + 6.0,
        "the plate should follow the cartridge bottom, not its top"
    );
    assert!(
        y + h <= OUT_H as f32 - MOUTH_H - 4.0,
        "the plate entered the slot mouth"
    );
}

#[test]
fn gba_gb_and_gbc_carts_all_keep_the_plate_below_the_cart() {
    for (name, cart) in [
        ("GBA", (180.0, 134.0, 360.0, 212.0)),
        ("GB", (204.0, 43.0, 312.0, 337.0)),
        ("GBC", (204.0, 43.0, 312.0, 337.0)),
    ] {
        let (_, y, _, h) = playtime_plate(cart, (200, 20));
        assert!(
            y >= cart.1 + cart.3 + 6.0,
            "{name}: the plate overlaps the cartridge"
        );
        assert!(
            y + h <= OUT_H as f32 - MOUTH_H - 4.0,
            "{name}: the plate enters the slot mouth"
        );
    }
}

#[test]
fn the_plate_is_clamped_inside_the_screen_margins() {
    let face = (200, 20);
    let (left, _, w, _) = playtime_plate((-200.0, 134.0, 240.0, 141.0), face);
    assert_eq!(left, 24.0);
    let (right, _, _, _) = playtime_plate((680.0, 134.0, 240.0, 141.0), face);
    assert_eq!(right + w, OUT_W as f32 - 24.0);
}

#[test]
fn a_cartridge_too_close_to_the_slot_gets_a_safe_plate_position() {
    let cart = (240.0, 350.0, 240.0, 141.0);
    let (_, y, _, h) = playtime_plate(cart, (200, 20));
    assert_eq!(y, OUT_H as f32 - MOUTH_H - 4.0 - 32.0);
    assert!(y + h <= OUT_H as f32 - MOUTH_H - 4.0);
}

#[test]
fn the_plate_never_overlaps_the_status_bar() {
    let cart = (240.0, 40.0, 240.0, 141.0);
    let (_, y, _, _) = playtime_plate(cart, (200, 20));
    assert!(y >= STATUS_H + 4.0);
}
