use slot_store::{Theme, ThemePreset};
use slot_ui::{set_theme, switch_style};

#[test]
fn the_web_palette_reaches_the_shared_menu_surface() {
    set_theme(Theme::preset(ThemePreset::Web));
    assert_eq!(switch_style::bg(), [0x1a, 0x1a, 0x2e, 0xff]);
    assert_eq!(switch_style::surface(), [0x2a, 0x2a, 0x40, 0xff]);
    assert_eq!(switch_style::accent(), [0x00, 0xd4, 0xaa]);
}

#[test]
fn the_warm_palette_keeps_selected_cards_readable() {
    let theme = Theme::preset(ThemePreset::Warm);
    let ink = theme.ink_selected;
    let selected = theme.selected;
    let luminance = |c: [u8; 3]| 0.2126 * c[0] as f32 + 0.7152 * c[1] as f32 + 0.0722 * c[2] as f32;
    assert!(
        (luminance(ink) - luminance([selected[0], selected[1], selected[2]])).abs() > 80.0,
        "selected text and card are too close"
    );
}
