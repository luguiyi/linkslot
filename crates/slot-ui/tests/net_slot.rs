use slot_ui::net_slot_face;

#[test]
fn the_net_slot_is_a_transparent_slot_face_with_a_sticker() {
    let face = net_slot_face();
    assert_eq!((face.w, face.h), (360, 212), "chooser card size changed");
    let at = |x: u32, y: u32| {
        let i = ((y * face.w + x) * 4) as usize;
        [face.rgba[i], face.rgba[i + 1], face.rgba[i + 2], face.rgba[i + 3]]
    };
    assert_eq!(at(0, 0)[3], 0, "the face must let the chooser plate through");
    assert_eq!(at(359, 0)[3], 0);
    assert!(at(180, 100)[3] > 200, "the slot body is missing");
    let sticker = at(100, 100);
    assert!(sticker[0] > 180 && sticker[1] > 170, "the sticker is missing");
    let cyan = (0..face.w)
        .flat_map(|x| (0..face.h).map(move |y| (x, y)))
        .filter(|(x, y)| {
            let c = at(*x, *y);
            c[3] > 180 && c[2] > 180 && c[1] > 150 && c[0] < 140
        })
        .count();
    assert!(cyan > 100, "the NET slot has no cyan contacts/LED");
}
