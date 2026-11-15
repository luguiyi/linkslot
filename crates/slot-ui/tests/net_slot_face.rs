use slot_ui::{module_label_face, net_card_face, net_card_label_face, net_module_face, net_slot_face, net_slot_face_with_sticker, NetSlotSticker};

#[test]
fn a_chooser_card_has_a_red_framed_ability_sticker() {
    let face = net_card_face("云存档");
    assert_eq!((face.w, face.h), (360, 212));
    let at = |x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2], face.rgba[i + 3]]
    };
    assert_eq!(at(0, 0)[3], 0, "the card row must stay transparent");
    assert!(at(180, 120)[3] > 180, "the green card body is missing");
    assert!(at(100, 42)[3] > 180, "the red sticker area is missing");
    let label = net_card_label_face("云存档");
    assert_eq!((label.w, label.h), (120, 22));
    assert!(label.rgba.chunks_exact(4).any(|px| px[3] > 0));
    if let Ok(path) = std::env::var("NET_CARD_RGBA") {
        std::fs::write(path, &face.rgba).unwrap();
    }
}

#[test]
fn a_chooser_row_is_an_attachable_module() {
    let face = net_module_face("GBAStation");
    assert_eq!((face.w, face.h), (360, 212));
    let at = |x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2], face.rgba[i + 3]]
    };
    assert_eq!(at(0, 0)[3], 0, "the module row must not paint a slot/card");
    assert!(at(180, 100)[3] > 180, "the module body is missing");
    let label = module_label_face("GBAStation");
    assert_eq!((label.w, label.h), (120, 20));
    assert!(label.rgba.chunks_exact(4).any(|px| px[3] > 0));
    if let Ok(path) = std::env::var("NET_MODULE_RGBA") {
        std::fs::write(path, &face.rgba).unwrap();
    }
}

#[test]
fn a_mod_sticker_uses_the_same_slot_and_paper() {
    let text = net_slot_face_with_sticker(NetSlotSticker::Text("同步 GBAStation"));
    assert_eq!((text.w, text.h), (360, 212));
    let at = |x: u32, y: u32| {
        let i = ((y * text.w + x) * 4) as usize;
        [
            text.rgba[i],
            text.rgba[i + 1],
            text.rgba[i + 2],
            text.rgba[i + 3],
        ]
    };
    assert!(at(100, 100)[3] > 180, "the shared sticker paper is missing");
    assert!(at(0, 0)[3] == 0, "the slot face must stay transparent outside");

    let art = [255u8, 80, 20, 255, 20, 255, 90, 255, 10, 100, 255, 255, 250, 250, 250, 255];
    let face = net_slot_face_with_sticker(NetSlotSticker::Art {
        rgba: &art,
        w: 2,
        h: 2,
    });
    assert_eq!((face.w, face.h), (360, 212));
    assert!(net_slot_face().rgba.len() == face.rgba.len());
}
