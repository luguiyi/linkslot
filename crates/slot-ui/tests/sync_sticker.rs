use slot_store::Platform;
use slot_ui::{
    labelled_cart_face_with_sticker, sync_cart_face_with_sticker, CartFace, CartSticker,
    DEFAULT_SHELL,
};

fn cart(label: &str, sticker: Option<CartSticker<'_>>) -> CartFace {
    labelled_cart_face_with_sticker(Platform::Gba, None, DEFAULT_SHELL, label, sticker)
}

fn has(face: &CartFace, test: impl Fn([u8; 4]) -> bool) -> bool {
    face.rgba
        .chunks_exact(4)
        .any(|px| test([px[0], px[1], px[2], px[3]]))
}

#[test]
fn the_builtin_sticker_is_a_dark_plate_with_a_red_rail_not_a_game_label() {
    let legacy = cart("同步云存档", None);
    let sticker = cart("同步云存档", Some(CartSticker::Text));
    assert_ne!(legacy.rgba, sticker.rgba, "the sticker changed nothing");

    // The project accent rail survives the paste.
    assert!(
        has(&sticker, |px| px[0] > 180 && px[1] < 70 && px[2] < 70),
        "no accent rail on the sticker"
    );
    // White ink, not the label's contrast colour.
    assert!(
        has(&sticker, |px| px[0] > 200 && px[1] > 200 && px[2] > 200),
        "no white sticker text"
    );
    // The dark plate fills most of the label.
    let dark = sticker
        .rgba
        .chunks_exact(4)
        .filter(|px| px[0] < 45 && px[1] < 45 && px[2] < 45)
        .count();
    assert!(dark > 1_000, "the sticker plate is too small ({dark} px)");
}

#[test]
fn a_mod_sticker_is_fitted_into_the_label_and_keeps_its_alpha() {
    let (aw, ah) = (100u32, 50u32);
    let mut art = vec![0u8; (aw * ah * 4) as usize];
    for px in art.chunks_exact_mut(4) {
        px.copy_from_slice(&[0x32, 0x8a, 0xe0, 0xff]);
    }
    let face = cart(
        "同步 SaveDeck",
        Some(CartSticker::Art {
            rgba: &art,
            w: aw,
            h: ah,
        }),
    );
    assert!(
        has(&face, |px| px[0] < 80 && px[1] > 100 && px[2] > 180),
        "the mod art never reached the cart"
    );
    assert_ne!(face.rgba, cart("同步 SaveDeck", None).rgba);
}

#[test]
fn the_fixed_sync_shell_is_not_a_plain_gba_cart() {
    let normal = labelled_cart_face_with_sticker(
        Platform::Gba,
        None,
        DEFAULT_SHELL,
        "同步云存档",
        Some(CartSticker::Text),
    );
    let sync =
        sync_cart_face_with_sticker(DEFAULT_SHELL, "同步云存档", Some(CartSticker::Text));
    assert_eq!((sync.w, sync.h), (normal.w, normal.h));
    assert_ne!(normal.rgba, sync.rgba, "the shell title did not change");
}

#[test]
fn only_the_cloud_sticker_gets_the_cloud_icon() {
    let cloud = cart("同步云存档", Some(CartSticker::Text));
    let plain = cart("同步云存档", Some(CartSticker::TextOnly));
    assert_ne!(cloud.rgba, plain.rgba, "the cloud icon was not drawn");
    let dark = |face: &CartFace| {
        face.rgba
            .chunks_exact(4)
            .filter(|px| px[0] < 45 && px[1] < 45 && px[2] < 45)
            .count()
    };
    assert!(
        dark(&plain) > dark(&cloud),
        "the icon did not replace plate pixels"
    );
}
