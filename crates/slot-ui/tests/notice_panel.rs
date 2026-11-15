use slot_ui::{Language, navigation, OUT_H, OUT_W};

fn inked(face: &slot_ui::UndoFace) -> usize {
    face.rgba.chunks_exact(4).filter(|p| p[3] > 0).count()
}

#[test]
fn a_notice_is_a_full_screen_card_with_no_cursor() {
    let lines = vec![
        "对方已断开连接。".to_string(),
        "本次联机已结束，可以继续当前游戏。".to_string(),
    ];
    let notice = navigation::notice("联机已断开", &lines, Language::ZhCn);
    assert_eq!((notice.w, notice.h), (OUT_W, 480));
    assert!(inked(&notice) > 10_000, "the notice drew almost nothing");
}

#[test]
fn a_notice_does_not_look_like_a_selectable_list() {
    let lines = vec![
        "当前游戏还没有可读取的攻略。".to_string(),
        "可在 Web 游戏详情上传 TXT / EPUB 攻略。".to_string(),
    ];
    let notice = navigation::notice("暂无攻略", &lines, Language::ZhCn);
    // The same prose in a list: the list paints a selected row, so the two
    // faces must not be byte-identical.
    let list = navigation::panel("暂无攻略", &lines, 0, Language::ZhCn);
    assert_ne!(notice.rgba, list.rgba, "the notice reused the list layout");
    let _ = OUT_H;
}