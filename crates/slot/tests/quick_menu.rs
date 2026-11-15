mod common;

use common::{
    app_booting_at, app_booting_with_clock, app_playing_in, tmp_root_with_carts, Clock,
    CLOCK_IS_SET,
};
use slot::app::{App, KeyboardPurpose, Phase, WifiUi};
use slot_input::{Action, Btn};
use slot_store::{read_slot_state, write_slot_state, Platform, Recent, ResumeKind, Shader, SlotState};
use slot_ui::{
    Draw, Icon, QuickMenuFaces, QuickRow, QuickValue, TexId, Toast, QUICK_PITCH, QUICK_TOP,
};
use tempfile::TempDir;

fn on_carousel_with(state: SlotState) -> (TempDir, App, Clock) {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_slot_state(
        d.path(),
        &SlotState {
            clock_set: true,
            ..state
        },
    )
    .expect("write slot.state");
    let (a, clock) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert!(
        matches!(a.phase(), Phase::Shelf),
        "not on the carousel: {:?}",
        a.phase()
    );
    (d, a, clock)
}

fn on_carousel() -> (TempDir, App, Clock) {
    on_carousel_with(SlotState::default())
}

fn press(a: &mut App, btn: Btn) {
    a.apply(Action::GbaDown(btn));
    a.apply(Action::GbaUp(btn));
}

fn open_at(a: &mut App, row: QuickRow) {
    let mut path = Vec::new();
    let mut current = row;
    while let Some(parent) = current.parent() {
        path.push(parent);
        current = parent;
    }
    path.reverse();

    // Leave whichever menu page the previous test step left open.
    while a.quick_menu().is_some() {
        a.apply(Action::GbaDown(Btn::B));
    }
    a.apply(Action::QuickMenu);
    for step in path {
        for _ in 0..step.position() {
            press(a, Btn::Down);
        }
        press(a, Btn::A);
    }
    for _ in 0..row.position() {
        press(a, Btn::Down);
    }
    assert_eq!(a.quick_menu(), Some(row), "the bar never reached {row:?}");
}

#[test]
fn the_theme_row_opens_a_list_and_applies_the_chosen_preset() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Theme);
    assert_eq!(a.quick_menu(), Some(QuickRow::Theme));

    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::ThemeDefault));
    assert_eq!(
        a.quick_value(QuickRow::ThemeDefault),
        Some(QuickValue::ThemeCurrent)
    );

    press(&mut a, Btn::Down);
    assert_eq!(a.quick_menu(), Some(QuickRow::ThemeWeb));
    assert_eq!(
        a.quick_value(QuickRow::ThemeWeb),
        Some(QuickValue::ThemePick)
    );
    press(&mut a, Btn::A);
    assert_eq!(
        a.quick_value(QuickRow::ThemeWeb),
        Some(QuickValue::ThemeCurrent)
    );
    let written = std::fs::read_to_string(d.path().join("linkSlot/Config/theme.txt")).unwrap();
    assert!(written.contains("preset web"), "{written}");

    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Theme));
    // 主题 now lives under 画面, so backing out lands on that page.
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Screen));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), None);
}

#[test]
fn cart_transfer_opens_the_matching_choices() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::CartTransfer);
    press(&mut a, Btn::A);

    let (title, rows, _) = a.utility_text().expect("no transfer menu");
    assert_eq!(title, "卡带传输模式");
    assert_eq!(
        rows,
        vec!["创建匹配", "加入匹配", "返回"]
    );
    press(&mut a, Btn::B);
    assert!(a.utility_text().is_none(), "B left the transfer menu open");
}

#[test]
fn menu_opens_the_quick_menu_with_its_top_row_selected_every_time() {
    let (_d, mut a, _) = on_carousel();
    a.apply(Action::QuickMenu);
    assert_eq!(a.quick_menu(), Some(QuickRow::MAIN[0]));
    press(&mut a, Btn::Down);
    a.apply(Action::QuickMenu);
    a.apply(Action::QuickMenu);
    assert_eq!(
        a.quick_menu(),
        Some(QuickRow::MAIN[0]),
        "the menu opened where it was last left"
    );
}

#[test]
fn menu_or_b_closes_it_back_onto_the_carousel_where_you_were() {
    for close in [Action::QuickMenu, Action::GbaDown(Btn::B)] {
        let (_d, mut a, _) = on_carousel();
        press(&mut a, Btn::Right);
        assert_eq!(
            a.selected_stem(),
            Some("Fusion"),
            "the carousel never moved"
        );
        a.apply(Action::QuickMenu);
        a.apply(close);
        assert!(
            matches!(a.phase(), Phase::Shelf),
            "{close:?} left {:?}",
            a.phase()
        );
        assert_eq!(a.selected_stem(), Some("Fusion"), "{close:?} lost the cart");
    }
}

#[test]
fn the_quick_menu_is_only_on_the_carousel() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let mut a = app_playing_in(d.path(), "Emerald");
    a.apply(Action::QuickMenu);
    assert!(
        matches!(a.phase(), Phase::Playing { .. }),
        "MENU raised the quick menu over a game: {:?}",
        a.phase()
    );
    assert_eq!(a.quick_menu(), None);
}

#[test]
fn up_and_down_move_the_bar_and_wrap_at_the_ends() {
    let (_d, mut a, _) = on_carousel();
    a.apply(Action::QuickMenu);
    press(&mut a, Btn::Up);
    assert_eq!(
        a.quick_menu(),
        Some(QuickRow::SystemSettings),
        "up from the top did not wrap to the bottom"
    );
    for want in [
        QuickRow::Screen,
        QuickRow::Shelf,
        QuickRow::Game,
        QuickRow::ButtonSettings,
        QuickRow::Network,
        QuickRow::SystemSettings,
        QuickRow::Screen,
    ] {
        press(&mut a, Btn::Down);
        assert_eq!(a.quick_menu(), Some(want));
    }
}

#[test]
fn fast_forward_steps_through_its_speeds_wraps_at_both_ends_and_saves_each_one() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::FastForward);
    for (btn, want) in [
        (Btn::Left, 4),
        (Btn::Left, 3),
        (Btn::Left, 2),
        (Btn::Left, 0),
        (Btn::Left, 6),
        (Btn::Right, 0),
        (Btn::Right, 2),
        (Btn::Right, 3),
        (Btn::Right, 4),
        (Btn::Right, 6),
        (Btn::Right, 0),
        (Btn::Right, 2),
    ] {
        press(&mut a, btn);
        assert_eq!(a.ff_speed(), want, "{btn:?} wanted {want}");
        assert_eq!(read_slot_state(d.path()).ff_speed, want, "not on the card");
        assert_eq!(
            a.quick_value(QuickRow::FastForward),
            QuickValue::speed(want)
        );
    }
}

#[test]
fn rumble_and_fast_forward_sound_flip_on_either_arrow_and_save() {
    let (d, mut a, _) = on_carousel();
    let card = |d: &TempDir| {
        let s = read_slot_state(d.path());
        (s.ff_sound, s.rumble)
    };
    open_at(&mut a, QuickRow::FastForwardSound);
    press(&mut a, Btn::Left);
    assert_eq!(card(&d), (true, true));
    assert_eq!(
        a.quick_value(QuickRow::FastForwardSound),
        Some(QuickValue::On)
    );
    press(&mut a, Btn::Right);
    assert_eq!(card(&d), (false, true));
    press(&mut a, Btn::Down);
    press(&mut a, Btn::Down);
    press(&mut a, Btn::Right);
    assert_eq!(card(&d), (false, false));
    assert!(!a.rumble_enabled());
    assert_eq!(a.quick_value(QuickRow::Rumble), Some(QuickValue::Off));
    press(&mut a, Btn::Left);
    assert_eq!(card(&d), (false, true));
}

#[test]
fn colour_correction_flips_on_either_arrow_and_saves() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ColourCorrection);
    assert!(
        !a.colour_correction(),
        "the row did not open on the default, which is off"
    );
    assert_eq!(
        a.quick_value(QuickRow::ColourCorrection),
        Some(QuickValue::Off)
    );
    for (btn, want) in [
        (Btn::Right, true),
        (Btn::Left, false),
        (Btn::Left, true),
        (Btn::Right, false),
    ] {
        press(&mut a, btn);
        assert_eq!(a.colour_correction(), want, "{btn:?}");
        assert_eq!(
            read_slot_state(d.path()).colour_correction,
            want,
            "{btn:?} never reached the card"
        );
        assert_eq!(
            a.quick_value(QuickRow::ColourCorrection),
            Some(QuickValue::flag(want))
        );
    }
}

#[test]
fn each_platform_load_switch_is_independent() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::LoadGb);
    // A fresh card has no per-platform switch: every machine follows the
    // default, so the row prints 默认.
    assert_eq!(a.quick_value(QuickRow::LoadGb), Some(QuickValue::Follow));
    assert!(read_slot_state(d.path()).resume(Platform::Gb, ResumeKind::Load));

    // 默认 -> 开
    press(&mut a, Btn::Right);
    let state = read_slot_state(d.path());
    assert_eq!(
        state.resume_override(Platform::Gb, ResumeKind::Load),
        Some(true)
    );
    assert_eq!(
        state.resume_override(Platform::Gba, ResumeKind::Load),
        None,
        "GBA got coupled"
    );
    assert_eq!(
        state.resume_override(Platform::Gbc, ResumeKind::Load),
        None,
        "GBC got coupled"
    );
    assert_eq!(
        state.resume_override(Platform::Gb, ResumeKind::Save),
        None,
        "the save switch got coupled to the load switch"
    );
    assert_eq!(a.quick_value(QuickRow::LoadGb), Some(QuickValue::On));

    // 开 -> 关 -> 默认 (the third step rejoins the default).
    press(&mut a, Btn::Right);
    assert_eq!(a.quick_value(QuickRow::LoadGb), Some(QuickValue::Off));
    press(&mut a, Btn::Right);
    assert_eq!(a.quick_value(QuickRow::LoadGb), Some(QuickValue::Follow));
    // Either arrow advances the same three-stop cycle.
    press(&mut a, Btn::Left);
    assert_eq!(a.quick_value(QuickRow::LoadGb), Some(QuickValue::On));
}

#[test]
fn the_two_defaults_drive_the_platforms_that_follow_them() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::DefaultLoad);
    assert_eq!(a.quick_value(QuickRow::DefaultLoad), Some(QuickValue::On));
    press(&mut a, Btn::Right);
    assert_eq!(a.quick_value(QuickRow::DefaultLoad), Some(QuickValue::Off));
    let state = read_slot_state(d.path());
    assert!(!state.resume_load_default, "the default was not saved");
    assert!(
        !state.resume(Platform::Gbc, ResumeKind::Load),
        "a following platform did not move with the default"
    );
    assert!(
        !state.resume(Platform::Fc, ResumeKind::Load),
        "the machine with no rows of its own must follow the default"
    );

    // An explicit switch wins over the default.
    open_at(&mut a, QuickRow::LoadGbc);
    press(&mut a, Btn::Right);
    assert_eq!(a.quick_value(QuickRow::LoadGbc), Some(QuickValue::On));
    let state = read_slot_state(d.path());
    assert!(state.resume(Platform::Gbc, ResumeKind::Load));
    assert!(!state.resume(Platform::Gb, ResumeKind::Load));

    // The save default is its own row.
    open_at(&mut a, QuickRow::DefaultSave);
    press(&mut a, Btn::Right);
    assert_eq!(a.quick_value(QuickRow::DefaultSave), Some(QuickValue::Off));
    assert!(!read_slot_state(d.path()).resume(Platform::Gb, ResumeKind::Save));
}

#[test]
fn the_save_page_returns_to_the_game_page() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::SaveGbc);
    // A fresh card has no switch of its own: it follows the default.
    assert_eq!(a.quick_value(QuickRow::SaveGbc), Some(QuickValue::Follow));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::SaveSettings));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Game));
}

#[test]
fn brightness_and_blue_light_rows_change_and_save() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Brightness);
    let before = read_slot_state(d.path()).brightness;
    press(&mut a, Btn::Right);
    assert_eq!(read_slot_state(d.path()).brightness, before + 1);
    assert_eq!(
        a.quick_value(QuickRow::Brightness),
        Some(QuickValue::level(before + 1))
    );
    press(&mut a, Btn::Left);
    assert_eq!(read_slot_state(d.path()).brightness, before);

    open_at(&mut a, QuickRow::BlueLight);
    let before = read_slot_state(d.path()).blue_light;
    press(&mut a, Btn::Right);
    assert_eq!(read_slot_state(d.path()).blue_light, before + 1);
    assert_eq!(
        a.quick_value(QuickRow::BlueLight),
        Some(QuickValue::level(before + 1))
    );
}

#[test]
fn shelf_key_rows_change_and_write_the_carousel_config() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ShelfPrev);
    assert_eq!(
        a.quick_value(QuickRow::ShelfPrev),
        Some(QuickValue::Control(slot_store::Device::L1))
    );
    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::ShelfPrev),
        Some(QuickValue::Control(slot_store::Device::R1))
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/carousel.txt")).unwrap();
    assert!(text.contains("previous_shelf="), "{text}");
}

#[test]
fn shortcut_rows_cycle_the_select_chord_and_write_the_shortcut_config() {
    let (d, mut a, _) = on_carousel();
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::R1)),
        "the menu did not open on the default Select chord"
    );
    assert_eq!(
        a.quick_value(QuickRow::ShortcutFastForward),
        Some(QuickValue::PlainShortcut(slot_store::Device::R2)),
        "a plain-button shortcut was not shown as a plain key"
    );

    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::L2))
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt"))
        .expect("the row did not write linkSlot/Config/shortcuts.txt");
    assert!(text.contains("save_state=select+l2\n"), "{text}");
    assert!(
        text.contains("fast_forward=r2\n"),
        "the untouched shortcuts were not written: {text}"
    );
    assert_eq!(text.lines().count(), slot_store::Shortcut::ALL.len());
    assert!(
        a.take_shortcut_reload(),
        "the running session was not told to reload the shortcut file"
    );

    // A reboot reads the remapped chord back from the card.
    let (mut b, _) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert_eq!(
        b.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::L2))
    );
    open_at(&mut b, QuickRow::ShortcutSaveState);
    press(&mut b, Btn::Left);
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=select+r1\n"), "{text}");
}

#[test]
fn game_button_rows_remap_the_in_game_controls() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ButtonLayout);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::ControlUp));
    assert_eq!(
        a.quick_value(QuickRow::ControlUp),
        Some(QuickValue::Control(slot_store::Device::Up))
    );
    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::ControlUp),
        Some(QuickValue::Control(slot_store::Device::Down))
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/controls.txt")).unwrap();
    assert!(text.contains("up=down\n"), "{text}");
    assert_eq!(a.toast(), Some(Toast::Duplicate));
    assert!(a.take_controls_reload());
}

#[test]
fn boot_restores_the_last_shelf_and_cart() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    write_slot_state(
        d.path(),
        &SlotState {
            clock_set: true,
            shelf: Some("gba".into()),
            shelf_cart: Some(Recent {
                platform: slot_store::Platform::Gba,
                stem: "Fusion".into(),
            }),
            ..SlotState::default()
        },
    )
    .unwrap();
    let (a, _) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert_eq!(a.selected_stem(), Some("Fusion"));
}

#[test]
fn recents_visibility_and_favorite_toggle_are_persisted() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Recents);
    assert!(!read_slot_state(d.path()).show_recents);
    press(&mut a, Btn::Right);
    assert!(read_slot_state(d.path()).show_recents);
    assert_eq!(
        a.quick_value(QuickRow::Recents),
        Some(QuickValue::On)
    );

    press(&mut a, Btn::B);
    press(&mut a, Btn::B);
    press(&mut a, Btn::B);
    assert!(!read_slot_state(d.path()).favorites.iter().any(|f| f.stem == a.selected_stem().unwrap()));
    a.apply(Action::FfStart);
    let starred = read_slot_state(d.path());
    assert!(starred.favorites.iter().any(|f| f.stem == a.selected_stem().unwrap()));
    a.apply(Action::FfStart);
    assert!(!read_slot_state(d.path()).favorites.iter().any(|f| f.stem == a.selected_stem().unwrap()));
}

#[test]
fn the_game_page_flips_turbo_and_rewind_and_saves_each() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Rewind);
    let card = |d: &TempDir| {
        let s = read_slot_state(d.path());
        (s.turbo, s.rewind)
    };
    assert_eq!(card(&d), (false, true));
    assert!(!a.turbo(), "turbo is on by default");
    press(&mut a, Btn::Right);
    assert_eq!(card(&d), (false, false));
    assert!(!a.may_rewind(), "rewind still offered with it off");
    // Turbo lives on the 按键 page now.
    open_at(&mut a, QuickRow::Turbo);
    press(&mut a, Btn::Right);
    assert_eq!(card(&d), (true, false));
    assert!(a.turbo());
    press(&mut a, Btn::Left);
    assert_eq!(card(&d), (false, false));
    assert!(!a.turbo());
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::ButtonSettings));
}

#[test]
fn screen_opens_a_page_of_its_own_and_b_comes_back_to_it() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Screen);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::DisplayPanel));
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::Brightness));
    for want in [QuickRow::BlueLight, QuickRow::Brightness] {
        press(&mut a, Btn::Down);
        assert_eq!(
            a.quick_menu(),
            Some(want),
            "the panel page did not wrap on itself"
        );
    }
    // The chain goes back one level at a time: panel -> 画面 -> the shelf.
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::DisplayPanel));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Screen));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), None);

    // And the effects page keeps the four shader rows together.
    open_at(&mut a, QuickRow::Screen);
    press(&mut a, Btn::A);
    press(&mut a, Btn::Down);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::GbaShader));
    for want in [
        QuickRow::GbShader,
        QuickRow::GbPalettes,
        QuickRow::ColourCorrection,
        QuickRow::GbaShader,
    ] {
        press(&mut a, Btn::Down);
        assert_eq!(a.quick_menu(), Some(want));
    }
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::DisplayEffects));
    press(&mut a, Btn::B);

    // 遮罩 is its own page under 画面: 画面 -> 遮罩 -> FC 遮罩.
    assert_eq!(a.quick_menu(), Some(QuickRow::Screen));
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::DisplayPanel));
    press(&mut a, Btn::Down);
    press(&mut a, Btn::Down);
    assert_eq!(a.quick_menu(), Some(QuickRow::Masks));
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::FcMask));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Masks));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Screen));
}

#[test]
fn each_platform_keeps_its_own_shader_wraps_and_starts_on_grid_and_simpletex() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::GbaShader);
    assert_eq!(a.quick_value(QuickRow::GbaShader), Some(QuickValue::Grid));
    assert_eq!(
        a.quick_value(QuickRow::GbShader),
        Some(QuickValue::Simpletex)
    );
    for (btn, want, shown) in [
        (Btn::Right, Shader::Dot, QuickValue::Dot),
        (Btn::Right, Shader::FourPass, QuickValue::FourPass),
        (Btn::Left, Shader::Dot, QuickValue::Dot),
        (Btn::Left, Shader::Grid, QuickValue::Grid),
        (Btn::Left, Shader::Lcd3x, QuickValue::Lcd3x),
        (Btn::Left, Shader::Off, QuickValue::Off),
    ] {
        press(&mut a, btn);
        assert_eq!(a.quick_value(QuickRow::GbaShader), Some(shown), "{btn:?}");
        assert_eq!(
            read_slot_state(d.path()).shader_gba,
            want,
            "{btn:?} never reached the card"
        );
    }
    assert_eq!(read_slot_state(d.path()).shader_gb, Shader::Simpletex);
    press(&mut a, Btn::Down);
    press(&mut a, Btn::Left);
    assert_eq!(read_slot_state(d.path()).shader_gb, Shader::Grid);
    assert_eq!(read_slot_state(d.path()).shader_gba, Shader::Off);
}

#[test]
fn colour_correction_leaves_the_settings_around_it_alone() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ColourCorrection);
    press(&mut a, Btn::Right);
    let s = read_slot_state(d.path());
    assert!(s.colour_correction, "the row never took");
    assert_eq!(
        (s.ff_speed, s.ff_sound, s.rumble),
        (
            SlotState::default().ff_speed,
            SlotState::default().ff_sound,
            SlotState::default().rumble
        ),
        "the row reached a setting that is not its own"
    );
}

#[test]
fn the_arrows_change_nothing_on_a_row_that_opens() {
    let (d, mut a, _) = on_carousel();
    let before = std::fs::read(d.path().join("linkSlot/Config/slot.state")).expect("read slot.state");
    for row in [QuickRow::DateTime, QuickRow::About] {
        open_at(&mut a, row);
        press(&mut a, Btn::Left);
        press(&mut a, Btn::Right);
        assert_eq!(a.quick_menu(), Some(row), "an arrow left {row:?}");
        a.apply(Action::QuickMenu);
    }
    assert_eq!(
        std::fs::read(d.path().join("linkSlot/Config/slot.state")).expect("read slot.state"),
        before,
        "an arrow on a row that opens wrote the card"
    );
}

#[test]
fn a_on_about_opens_the_label_and_b_comes_back_to_the_menu() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::About);
    press(&mut a, Btn::A);
    assert!(matches!(a.phase(), Phase::About), "{:?}", a.phase());
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::About));
}

#[test]
fn a_on_date_and_time_opens_the_clock_at_the_time_it_already_has() {
    let (_d, mut a, _) = on_carousel_with(SlotState {
        utc_offset_min: -300,
        ..SlotState::default()
    });
    open_at(&mut a, QuickRow::DateTime);
    press(&mut a, Btn::A);
    let picker = a.picker().expect("A on Date & Time did not open the clock");
    assert_eq!(picker.offset_min(), -300, "the offset started over");
    assert_eq!(
        picker.secs(),
        CLOCK_IS_SET - CLOCK_IS_SET % 60,
        "the picker does not show the minute the clock is in"
    );
}

#[test]
fn confirming_the_clock_from_the_menu_sets_it_and_comes_back_to_the_menu() {
    let (d, mut a, clock) = on_carousel_with(SlotState {
        utc_offset_min: -300,
        ..SlotState::default()
    });
    open_at(&mut a, QuickRow::DateTime);
    press(&mut a, Btn::A);
    press(&mut a, Btn::Up);
    for _ in 0..5 {
        press(&mut a, Btn::Right);
    }
    press(&mut a, Btn::Down);
    let changed = a.picker().expect("not on the clock").secs() - (CLOCK_IS_SET - CLOCK_IS_SET % 60);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::DateTime), "{:?}", a.phase());
    assert_eq!(
        clock.get(),
        CLOCK_IS_SET + changed,
        "the platform clock was not set"
    );
    let s = read_slot_state(d.path());
    assert_eq!(s.utc_offset_min, -330, "the offset was not saved");
    assert!(s.clock_set);
}

#[test]
fn b_on_the_clock_from_the_menu_comes_back_without_changing_anything() {
    let (d, mut a, clock) = on_carousel_with(SlotState {
        utc_offset_min: -300,
        ..SlotState::default()
    });
    open_at(&mut a, QuickRow::DateTime);
    press(&mut a, Btn::A);
    press(&mut a, Btn::Up);
    for _ in 0..5 {
        press(&mut a, Btn::Right);
    }
    press(&mut a, Btn::Down);
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::DateTime), "{:?}", a.phase());
    assert_eq!(clock.get(), CLOCK_IS_SET, "B set the clock");
    assert_eq!(
        read_slot_state(d.path()).utc_offset_min,
        -300,
        "B saved the offset"
    );
}

#[test]
fn the_first_boot_clock_still_has_no_way_back() {
    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut a, _) = app_booting_with_clock(d.path());
    press(&mut a, Btn::B);
    a.apply(Action::QuickMenu);
    assert!(
        matches!(a.phase(), Phase::SetClock { .. }),
        "left the first boot clock: {:?}",
        a.phase()
    );
}

#[test]
fn brightness_and_volume_still_answer_over_the_quick_menu() {
    let (d, mut a, _) = on_carousel();
    let icons: Vec<TexId> = (0..Icon::ALL.len())
        .map(|i| TexId::from_raw(700 + i))
        .collect();
    a.set_icon_faces(icons.clone());
    open_at(&mut a, QuickRow::FastForward);
    let before = read_slot_state(d.path());
    a.apply(Action::BrightnessUp);
    a.apply(Action::VolumeDown);
    a.flush_state();
    let after = read_slot_state(d.path());
    assert_eq!(
        (after.brightness, after.volume),
        (before.brightness + 1, before.volume - 5)
    );
    assert_eq!(
        a.quick_menu(),
        Some(QuickRow::FastForward),
        "a level moved the menu"
    );
    let out = frame(&a);
    assert!(
        out.iter()
            .any(|d| matches!(*d, Draw::Tex { tex, .. } if icons.contains(&tex))),
        "the level's bar is not drawn over the menu"
    );
}

fn fake_faces(a: &mut App) {
    let id = TexId::from_raw;
    a.set_quick_menu_faces(QuickMenuFaces {
        cards: [(id(600), 624, 42), (id(601), 624, 42)],
        labels: (0..QuickRow::ALL.len())
            .map(|i| [(id(100 + i), 200, 40), (id(150 + i), 200, 40)])
            .collect(),
        values: (0..QuickValue::ALL.len())
            .map(|i| [(id(200 + i), 60, 40), (id(210 + i), 60, 40)])
            .collect(),
        carets: [
            [(id(300), 10, 40), (id(302), 10, 40)],
            [(id(301), 10, 40), (id(303), 10, 40)],
        ],
        icons: (0..QuickRow::ALL.len())
            .map(|i| [(id(700 + i), 24, 24), (id(750 + i), 24, 24)])
            .collect(),
        titles: [
            (id(800), 120, 40),
            (id(810), 160, 40),
            (id(811), 160, 40),
            (id(812), 160, 40),
            (id(813), 160, 40),
            (id(814), 160, 40),
            (id(801), 160, 40),
            (id(802), 160, 40),
            (id(803), 160, 40),
            (id(804), 160, 40),
            (id(805), 160, 40),
            (id(806), 160, 40),
            (id(807), 160, 40),
            (id(808), 160, 40),
            (id(809), 160, 40),
            (id(815), 160, 40),
        ],
        legend: [(id(400), 70), (id(401), 110), (id(402), 80)],
    });
    a.set_quick_clock_faces((id(500), 150, 40), (id(501), 150, 40));
}

fn value(v: QuickValue, lit: bool) -> usize {
    if lit {
        210 + v.index()
    } else {
        200 + v.index()
    }
}

fn frame(a: &App) -> Vec<Draw> {
    let mut out = Vec::new();
    a.draw(&mut out);
    out
}

fn placed(out: &[Draw], id: usize) -> Option<[f32; 4]> {
    out.iter().find_map(|d| match *d {
        Draw::Tex {
            x, y, w, h, tex, ..
        } if tex == TexId::from_raw(id) => Some([x, y, w, h]),
        _ => None,
    })
}

fn drawn(out: &[Draw], id: usize) -> bool {
    placed(out, id).is_some()
}

#[test]
fn the_arrows_stand_only_around_the_selected_rows_value() {
    let (_d, mut a, _) = on_carousel();
    fake_faces(&mut a);
    open_at(&mut a, QuickRow::FastForward);
    let out = frame(&a);
    assert!(
        drawn(&out, 302) && drawn(&out, 303),
        "no arrows on Fast Forward"
    );
    assert!(
        drawn(&out, value(QuickValue::Speed6, true)),
        "6× is not lit"
    );
    assert!(
        drawn(&out, value(QuickValue::Off, false)),
        "sound's OFF is not grey"
    );
    assert!(
        drawn(&out, value(QuickValue::On, false)),
        "rumble's ON is not grey"
    );
    press(&mut a, Btn::B);
    let out = frame(&a);
    // DateTime is no longer on the main page; it now lives in 系统设置.
    assert!(!drawn(&out, 500), "DateTime leaked onto the main page");

    for _ in QuickRow::Game.position()..QuickRow::SystemSettings.position() {
        press(&mut a, Btn::Down);
    }
    press(&mut a, Btn::A);
    let out = frame(&a);
    assert!(
        !drawn(&out, 302),
        "a left adjustment arrow on a row that opens"
    );
    assert!(drawn(&out, 303), "the row that opens has no chevron");
    assert!(drawn(&out, 501), "the date and time in hand is not lit");
}

#[test]
fn mod_settings_lists_and_toggles_installed_mods() {
    let (d, mut a, _) = on_carousel();
    for (id, name, enabled) in [
        ("alpha", "Alpha Mod", true),
        ("beta", "Beta Mod", false),
    ] {
        let dir = d.path().join("linkSlot/mods").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("mod.json"),
            format!(
                r#"{{"schema":2,"name":"{name}","version":"1.0.0","enabled":{enabled}}}"#
            ),
        )
        .unwrap();
    }

    open_at(&mut a, QuickRow::Mods);
    assert!(a.mods_ui.is_none(), "A was not pressed yet");
    press(&mut a, Btn::A);
    let ui = a.mods_ui.as_ref().expect("mod list");
    assert_eq!(ui.rows.len(), 2);
    assert_eq!(ui.rows[0].id, "alpha");
    assert!(ui.rows[0].enabled);
    assert!(!ui.rows[1].enabled);

    press(&mut a, Btn::A);
    assert!(!slot_mods::read_mod(d.path(), "alpha").unwrap().entry.enabled);
    assert!(!a.mods_ui.as_ref().unwrap().rows[0].enabled);

    press(&mut a, Btn::Down);
    press(&mut a, Btn::A);
    assert!(slot_mods::read_mod(d.path(), "beta").unwrap().entry.enabled);
    assert!(a.mods_ui.as_ref().unwrap().rows[1].enabled);

    press(&mut a, Btn::B);
    assert!(a.mods_ui.is_none());
    assert_eq!(a.quick_menu(), Some(QuickRow::Mods));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::SystemSettings));
    press(&mut a, Btn::B);
    assert!(matches!(a.phase(), Phase::Shelf));
}

/// Write a mod manifest, optionally with an explicit priority.
fn write_mod(d: &TempDir, id: &str, name: &str, order: Option<i32>) {
    let dir = d.path().join("linkSlot/mods").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let order = order.map_or(String::new(), |order| format!(r#","order":{order}"#));
    std::fs::write(
        dir.join("mod.json"),
        format!(r#"{{"schema":2,"name":"{name}","version":"1.0.0","enabled":true{order}}}"#),
    )
    .unwrap();
}

/// The list is the priority queue: the shelf long-press runs the top-most mod
/// that asks for it, so moving a row with ←/→ must persist a new order and
/// bring the mods back in that order.
#[test]
fn mod_settings_reorders_the_priority_queue() {
    let (d, mut a, _) = on_carousel();
    write_mod(&d, "alpha", "Alpha Mod", None);
    write_mod(&d, "beta", "Beta Mod", Some(20));

    open_at(&mut a, QuickRow::Mods);
    press(&mut a, Btn::A);
    let ui = a.mods_ui.as_ref().expect("mod list");
    assert_eq!(
        ui.rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["beta", "alpha"],
        "the list did not start in priority order"
    );

    // Select the lower mod and send it up with ←.
    press(&mut a, Btn::Down);
    press(&mut a, Btn::Left);
    let ui = a.mods_ui.as_ref().unwrap();
    assert_eq!(ui.selected, 0, "the moved mod was not kept selected");
    assert_eq!(
        ui.rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    assert_eq!(slot_mods::read_mod(d.path(), "alpha").unwrap().entry.order, 0);
    assert_eq!(
        slot_mods::read_mod(d.path(), "beta").unwrap().entry.order,
        slot_mods::ORDER_STEP
    );

    // The top of the queue has nowhere to go up to.
    press(&mut a, Btn::Left);
    assert_eq!(a.mods_ui.as_ref().unwrap().selected, 0);
    assert_eq!(
        a.mods_ui
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );

    // And → pushes it back down, renumbering the queue again.
    press(&mut a, Btn::Right);
    assert_eq!(
        a.mods_ui
            .as_ref()
            .unwrap()
            .rows
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        ["beta", "alpha"]
    );
    assert_eq!(
        slot_mods::read_mod(d.path(), "beta").unwrap().entry.order,
        0
    );
}

#[test]
fn mod_settings_shows_an_empty_list() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Mods);
    press(&mut a, Btn::A);
    let ui = a.mods_ui.as_ref().expect("mod list");
    assert!(ui.rows.is_empty());
    assert!(ui.error.is_none());
}

#[test]
fn the_selected_card_sits_behind_the_selected_row() {
    let (_d, mut a, _) = on_carousel();
    fake_faces(&mut a);
    a.apply(Action::QuickMenu);
    for row in QuickRow::MAIN {
        let cards: Vec<_> = frame(&a)
            .into_iter()
            .filter_map(|d| match d {
                Draw::Tex {
                    x, y, w, h, tex, ..
                } if tex == TexId::from_raw(601) => Some([x, y, w, h]),
                _ => None,
            })
            .collect();
        let offset = row.position().saturating_sub(7);
        let top = QUICK_TOP + QUICK_PITCH * (row.position() - offset) as f32;
        assert_eq!(
            cards,
            vec![[48.0, top, 624.0, 42.0]],
            "{row:?}"
        );
        press(&mut a, Btn::Down);
    }
}

#[test]
fn labels_start_and_values_end_inside_the_selected_card() {
    let (_d, mut a, _) = on_carousel();
    fake_faces(&mut a);
    open_at(&mut a, QuickRow::FastForward);
    let out = frame(&a);
    let card_right = 672.0;
    for row in QuickRow::GAME.iter().take(8).copied() {
        let id = if row == QuickRow::FastForward {
            150 + row.index()
        } else {
            100 + row.index()
        };
        let [x, ..] = placed(&out, id).expect("a label was not drawn");
        assert_eq!(x, 104.0, "{row:?}'s label");
    }
    let [x, _, w, _] = placed(&out, value(QuickValue::On, false)).expect("rumble's value");
    assert_eq!(x + w, card_right - 24.0, "an unselected value");
    let [x, _, w, _] = placed(&out, 303).expect("the right arrow");
    assert_eq!(x + w, card_right - 24.0, "the arrow around the selected value");
}

#[test]
fn only_the_clock_from_the_menu_offers_b_back() {
    let (_d, mut a, _) = on_carousel();
    fake_faces(&mut a);
    a.set_clock_faces(TexId::from_raw(600), TexId::from_raw(601));
    open_at(&mut a, QuickRow::DateTime);
    press(&mut a, Btn::A);
    let out = frame(&a);
    assert!(drawn(&out, 601), "the clock lost its own key");
    assert!(
        drawn(&out, 400),
        "the clock from the menu does not offer B BACK"
    );

    let d = tmp_root_with_carts(&["Emerald", "Fusion"]);
    let (mut first, _) = app_booting_with_clock(d.path());
    fake_faces(&mut first);
    first.set_clock_faces(TexId::from_raw(600), TexId::from_raw(601));
    let out = frame(&first);
    assert!(drawn(&out, 601), "the first boot clock lost its key");
    assert!(
        !drawn(&out, 400),
        "the first boot clock offers a way back it does not have"
    );
}

#[test]
fn gb_palettes_cycles_auto_and_the_forty_eight_palettes() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::GbPalettes);
    assert_eq!(a.quick_value(QuickRow::GbPalettes), Some(QuickValue::Off));
    press(&mut a, Btn::Right);
    assert!(read_slot_state(d.path()).gb_palettes);
    assert_eq!(
        a.quick_value(QuickRow::GbPalettes),
        Some(QuickValue::Palette(slot_store::GbPalette::at(0)))
    );
    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::GbPalettes),
        Some(QuickValue::Palette(slot_store::GbPalette::at(1)))
    );
    press(&mut a, Btn::Left);
    assert_eq!(
        a.quick_value(QuickRow::GbPalettes),
        Some(QuickValue::Palette(slot_store::GbPalette::at(0)))
    );
    press(&mut a, Btn::Left);
    assert!(!read_slot_state(d.path()).gb_palettes);
    assert_eq!(a.quick_value(QuickRow::GbPalettes), Some(QuickValue::Off));
}

#[test]
fn the_network_page_opens_a_second_page_of_wifi_and_web() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Network);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::Wifi));
    press(&mut a, Btn::Down);
    assert_eq!(a.quick_menu(), Some(QuickRow::Web));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Network));
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), None, "B did not leave the menu");
}

#[test]
fn the_network_page_has_sync_content_as_its_fourth_row() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Network);
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::Wifi));
    for expected in [
        QuickRow::Web,
        QuickRow::SyncStorage,
        QuickRow::SyncContent,
        QuickRow::CartTransfer,
    ] {
        press(&mut a, Btn::Down);
        assert_eq!(a.quick_menu(), Some(expected));
    }
    press(&mut a, Btn::Down);
    assert_eq!(
        a.quick_menu(),
        Some(QuickRow::Wifi),
        "the five network rows should wrap"
    );
    // The main menu now has six rows; the three button pages live under
    // 按键设置 and 日期/关于 live under 系统设置.
    press(&mut a, Btn::B);
    assert_eq!(a.quick_menu(), Some(QuickRow::Network));
    press(&mut a, Btn::B);
    a.apply(Action::QuickMenu);
    let mut labels = Vec::new();
    for _ in 0..6 {
        labels.push(a.quick_menu().expect("row").label(a.language()));
        press(&mut a, Btn::Down);
    }
    assert_eq!(a.quick_menu(), Some(QuickRow::MAIN[0]));
    assert_eq!(labels.len(), 6);
}

#[test]
fn sync_content_cycles_three_modes_and_saves_immediately() {
    let (d, mut a, _) = on_carousel();
    assert_eq!(
        slot::linkcfg::read(d.path()).sync.mode,
        slot::linkcfg::SyncMode::All,
        "the default sync mode should be 全部"
    );
    open_at(&mut a, QuickRow::SyncContent);
    assert_eq!(
        a.quick_value(QuickRow::SyncContent),
        Some(QuickValue::SyncAll)
    );

    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::SyncContent),
        Some(QuickValue::SyncState)
    );
    assert_eq!(
        slot::linkcfg::read(d.path()).sync.mode,
        slot::linkcfg::SyncMode::State,
        "the mode must be saved immediately"
    );

    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::SyncContent),
        Some(QuickValue::SyncSav)
    );
    assert_eq!(
        slot::linkcfg::read(d.path()).sync.mode,
        slot::linkcfg::SyncMode::Sav
    );

    press(&mut a, Btn::Right);
    assert_eq!(
        a.quick_value(QuickRow::SyncContent),
        Some(QuickValue::SyncAll)
    );
    assert_eq!(
        slot::linkcfg::read(d.path()).sync.mode,
        slot::linkcfg::SyncMode::All
    );

    press(&mut a, Btn::Left);
    assert_eq!(
        a.quick_value(QuickRow::SyncContent),
        Some(QuickValue::SyncSav),
        "left should wrap backwards"
    );

    // A has no subpage on this row.
    press(&mut a, Btn::A);
    assert_eq!(a.quick_menu(), Some(QuickRow::SyncContent));
}

#[test]
fn saved_wifi_is_shown_first_and_can_be_selected() {
    let (d, mut a, _) = on_carousel();
    slot::linkcfg::write_wifi(d.path(), "home", "secret123").unwrap();
    open_at(&mut a, QuickRow::Wifi);
    press(&mut a, Btn::A);
    let ui = a.wifi_ui.as_ref().expect("WiFi page");
    assert!(
        ui.saved.iter().any(|network| network.ssid == "home"),
        "saved list missing home: {:?}",
        ui.saved
    );
    assert!(
        ui.rows.iter().any(|row| row.saved && row.ssid == "home"),
        "saved row missing: {:?}",
        ui.rows
    );
    let index = ui
        .rows
        .iter()
        .position(|row| row.saved && row.ssid == "home")
        .unwrap();
    assert_eq!(index, 0, "saved network should be listed first");
}

#[test]
fn closing_search_keeps_the_shelf_draw_items() {
    let (_d, mut a, _) = on_carousel();
    let mut before = Vec::new();
    a.draw(&mut before);
    a.apply(Action::GbaDown(Btn::X));
    a.apply(Action::GbaUp(Btn::X));
    type_search_text(&mut a, "a");
    a.apply(Action::GbaDown(Btn::X));
    let mut after = Vec::new();
    a.draw(&mut after);
    assert!(
        after.len() >= before.len() / 2,
        "search exit lost most shelf draws: before={} after={}",
        before.len(),
        after.len()
    );
}

fn type_search_text(a: &mut App, text: &str) {
    for ch in text.chars() {
        let kb = a.keyboard.as_ref().expect("search keyboard");
        let rows = slot_ui::keyboard::rows(kb.cursor.layout);
        let label = ch.to_string();
        let (row, col) = rows
            .iter()
            .enumerate()
            .find_map(|(row, keys)| keys.iter().position(|key| *key == label).map(|col| (row, col)))
            .unwrap_or_else(|| panic!("no key {ch:?}"));
        let layout = kb.cursor.layout;
        a.keyboard.as_mut().unwrap().cursor = slot_ui::keyboard::Cursor { layout, row, col };
        a.apply(Action::GbaDown(Btn::A));
    }
}

#[test]
fn sync_method_page_requires_a_test_before_switching() {
    let (d, mut a, _) = on_carousel();
    let edit = |id: &str, name: &str, url: &str, root: &str, password: &str| {
        slot::linkcfg::StorageEdit {
            id: id.to_string(),
            name: name.to_string(),
            kind: "webdav".to_string(),
            url: url.to_string(),
            remote_root: root.to_string(),
            username: format!("{id}-user"),
            password: Some(password.to_string()),
            ca_cert: String::new(),
            insecure: false,
        }
    };
    slot::linkcfg::save_storages(
        d.path(),
        "home",
        &[
            edit("home", "家里", "https://home.example/dav", "/home", "home-pass"),
            edit("work", "公司", "https://work.example/dav", "/work", "work-pass"),
        ],
        false,
    )
    .expect("save sync methods");
    open_at(&mut a, QuickRow::SyncStorage);
    press(&mut a, Btn::A);
    assert!(a.sync_ui.is_some(), "sync method page did not open");
    assert!(!a.sync_test_ready(), "current profile should not need a test");
    press(&mut a, Btn::Down);
    assert!(a.sync_test_ready(), "a different profile can be tested");
    assert!(!a.sync_can_switch(), "switch must stay locked before testing");
    a.sync_ui.as_mut().unwrap().tested = Some("work".to_string());
    assert!(a.sync_can_switch(), "successful test must unlock switching");
    a.apply(Action::GbaDown(Btn::X));
    assert_eq!(
        slot::linkcfg::read(d.path()).active_storage,
        "work",
        "switching did not persist the active profile"
    );
}

#[test]
fn first_use_sync_method_page_offers_to_start_the_web_service() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::SyncStorage);
    press(&mut a, Btn::A);
    assert!(a.sync_ui.is_some(), "sync method page did not open");
    assert!(a.sync_needs_setup(), "an empty default profile is not first use");
    assert_eq!(a.sync_ui.as_ref().unwrap().profiles.len(), 0);
    assert!(!a.sync_test_ready());
    assert!(!a.sync_can_switch());

    press(&mut a, Btn::A);
    assert!(a.sync_ui.is_none(), "the setup page did not close");
    assert!(
        a.web_popup.is_some(),
        "A on the first-use page did not start the Web service"
    );
}

#[test]
fn x_on_a_saved_wifi_opens_the_password_editor() {
    let (_d, mut a, _) = on_carousel();
    a.wifi_ui = Some(WifiUi {
        status: "选择网络".into(),
        connected: None,
        error: Some("关联 Wi-Fi 超时；可按 X 重新输入密码".into()),
        rows: vec![slot_ui::network::WifiEntry {
            ssid: "home".into(),
            signal: -50,
            secured: true,
            saved: true,
        }],
        saved: vec![slot::linkcfg::SavedWifi {
            ssid: "home".into(),
            password_set: true,
        }],
        selected: 2,
        offset: 0,
        manual: true,
        connecting: None,
        job: None,
        status_job: None,
        radio_on: true,
        radio_known: true,
        radio_job: None,
        radio_pending: None,
        auto_off_minutes: 10,
    });
    press(&mut a, Btn::X);
    let keyboard = a.keyboard.as_ref().expect("saved password editor did not open");
    assert!(matches!(
        keyboard.purpose,
        KeyboardPurpose::WifiPassword { ref ssid } if ssid == "home"
    ));
}

#[test]
fn refreshing_wifi_turns_the_radio_on_when_it_was_never_connected() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Wifi);
    a.wifi_ui = Some(WifiUi {
        status: "WiFi 已关闭".into(),
        connected: None,
        error: None,
        rows: Vec::new(),
        saved: Vec::new(),
        selected: 2,
        offset: 0,
        manual: true,
        connecting: None,
        job: None,
        status_job: None,
        radio_on: false,
        radio_known: true,
        radio_job: None,
        radio_pending: None,
        auto_off_minutes: 10,
    });
    press(&mut a, Btn::Y);
    let ui = a.wifi_ui.as_ref().expect("WiFi page closed");
    assert_eq!(ui.radio_pending, Some(true));
    assert!(ui.radio_job.is_some(), "refresh did not start the radio");
    assert!(ui.status.contains("开启"));
}

#[test]
fn wifi_manual_entry_opens_the_ssid_keyboard_and_cancels() {
    let (_d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::Wifi);
    a.wifi_ui = Some(WifiUi {
        status: "选择网络".into(),
        connected: None,
        error: None,
        rows: Vec::new(),
        saved: Vec::new(),
        selected: 2,
        offset: 0,
        manual: true,
        connecting: None,
        job: None,
        status_job: None,
        radio_on: true,
        radio_known: true,
        radio_job: None,
        radio_pending: None,
        auto_off_minutes: 10,
    });
    press(&mut a, Btn::A);
    let kb = a.keyboard.as_ref().expect("the manual row did not open a keyboard");
    assert!(matches!(kb.purpose, KeyboardPurpose::WifiSsid));
    press(&mut a, Btn::A);
    assert_eq!(a.keyboard.as_ref().unwrap().text, "q");
    press(&mut a, Btn::B);
    assert!(a.keyboard.as_ref().unwrap().text.is_empty());
    press(&mut a, Btn::X);
    assert!(a.keyboard.is_none(), "X did not cancel the keyboard");
    assert!(a.wifi_ui.is_some(), "cancelling the keyboard left the Wi-Fi page");
    press(&mut a, Btn::B);
    assert!(a.wifi_ui.is_none(), "B did not leave the Wi-Fi page");
}

#[test]
fn a_shortcut_can_be_switched_off_and_the_page_can_reset_them_all() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ShortcutSaveState);

    // Walking left past the Select chords lands on 无, and that is written down.
    let mut reached = false;
    for _ in 0..=slot_store::Device::ALL.len() {
        press(&mut a, Btn::Left);
        if a.quick_value(QuickRow::ShortcutSaveState) == Some(QuickValue::Unbound) {
            reached = true;
            break;
        }
    }
    assert!(reached, "the row never reached 无");
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=none\n"), "{text}");
    assert!(a.take_shortcut_reload());

    // A reboot keeps it off instead of falling back to the factory chord.
    let (mut b, _) = app_booting_at(d.path(), CLOCK_IS_SET);
    assert_eq!(
        b.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Unbound)
    );

    // 重置快捷键 puts every chord back to its default.
    open_at(&mut a, QuickRow::ShortcutReset);
    press(&mut a, Btn::A);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::R1))
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=select+r1\n"), "{text}");
    assert!(a.take_shortcut_reload());
}
#[test]
fn a_shortcut_row_takes_the_key_the_player_presses() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::A);
    let (title, rows, _) = a.utility_text().expect("the mapping popup did not open");
    assert_eq!(title, "设置「保存状态」");
    assert!(rows.iter().any(|row| row.contains("按下一个按键")), "{rows:?}");

    // A plain press becomes a plain key.
    press(&mut a, Btn::R1);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::PlainShortcut(slot_store::Device::R1))
    );
    assert_eq!(a.utility_text(), None, "the popup stayed up");
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=r1\n"), "{text}");
    assert!(a.take_shortcut_reload());

    // SELECT held makes a chord.
    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::A);
    a.apply(Action::GbaDown(Btn::Select));
    press(&mut a, Btn::R1);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::R1))
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=select+r1\n"), "{text}");
}

#[test]
fn a_clashing_key_does_not_change_the_shortcut() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::A);
    // R2 is already 按住快进's key.
    press(&mut a, Btn::R2);
    assert_eq!(a.toast(), Some(Toast::Duplicate));
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::R1)),
        "a clashing key was accepted"
    );
    assert!(
        a.utility_text().is_some(),
        "the popup closed on a clash instead of asking again"
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap_or_default();
    assert!(!text.contains("save_state="), "a clash was written: {text}");
}

#[test]
fn m_without_a_key_asks_before_clearing_the_shortcut() {
    let (d, mut a, _) = on_carousel();
    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::A);
    a.apply(Action::QuickMenu);
    let (title, rows, _) = a.utility_text().expect("no 设为无 question");
    assert_eq!(title, "设为无？");
    assert!(rows.iter().any(|row| row.contains("继续")), "{rows:?}");

    // B keeps the old chord.
    press(&mut a, Btn::B);
    assert_eq!(a.utility_text(), None);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Shortcut(slot_store::Device::R1))
    );

    // A clears it, and that is written down.
    open_at(&mut a, QuickRow::ShortcutSaveState);
    press(&mut a, Btn::A);
    a.apply(Action::QuickMenu);
    press(&mut a, Btn::A);
    assert_eq!(
        a.quick_value(QuickRow::ShortcutSaveState),
        Some(QuickValue::Unbound)
    );
    let text = std::fs::read_to_string(d.path().join("linkSlot/Config/shortcuts.txt")).unwrap();
    assert!(text.contains("save_state=none\n"), "{text}");
}
#[test]
fn the_gb_gbc_and_sfc_mask_rows_cycle_their_own_files() {
    let (d, mut a, _) = on_carousel();
    std::fs::create_dir_all(d.path().join("Overlay/GB")).unwrap();
    std::fs::create_dir_all(d.path().join("Overlay/GBC")).unwrap();
    std::fs::create_dir_all(d.path().join("Overlay/SFC")).unwrap();
    std::fs::write(d.path().join("Overlay/GB/GB01.png"), b"x").unwrap();
    std::fs::write(d.path().join("Overlay/GBC/GBC01.png"), b"x").unwrap();
    std::fs::write(d.path().join("Overlay/SFC/SFC01.png"), b"x").unwrap();
    a.refresh_masks();

    open_at(&mut a, QuickRow::GbMask);
    assert_eq!(a.mask_label_for(Platform::Gb), "无");
    press(&mut a, Btn::Right);
    assert_eq!(a.mask_label_for(Platform::Gb), "GB01.png");
    assert_eq!(read_slot_state(d.path()).gb_mask.as_deref(), Some("GB01.png"));
    assert_eq!(a.mask_label_for(Platform::Gbc), "无");

    open_at(&mut a, QuickRow::GbcMask);
    press(&mut a, Btn::Right);
    assert_eq!(a.mask_label_for(Platform::Gbc), "GBC01.png");
    assert_eq!(
        read_slot_state(d.path()).gbc_mask.as_deref(),
        Some("GBC01.png")
    );
    assert_eq!(a.mask_label_for(Platform::Gb), "GB01.png");

    open_at(&mut a, QuickRow::SfcMask);
    assert_eq!(a.mask_label_for(Platform::Sfc), "无");
    press(&mut a, Btn::Right);
    assert_eq!(a.mask_label_for(Platform::Sfc), "SFC01.png");
    assert_eq!(
        read_slot_state(d.path()).sfc_mask.as_deref(),
        Some("SFC01.png")
    );
    assert_eq!(a.mask_label_for(Platform::Gbc), "GBC01.png");

    a.set_platform(Platform::Sfc);
    assert_eq!(a.mask_dir_name(), Some("SFC"));
    assert_eq!(a.current_mask(), Some("SFC01.png"));
    assert!(a.mask_wanted(), "the SFC mask is selected but not wanted");
}

#[test]
fn the_fc_mask_row_cycles_the_files_on_the_card() {
    let (d, mut a, _) = on_carousel();
    std::fs::create_dir_all(d.path().join("Overlay/FC")).unwrap();
    std::fs::write(d.path().join("Overlay/FC/FC01.png"), b"x").unwrap();
    std::fs::write(d.path().join("Overlay/FC/FC02.png"), b"x").unwrap();
    a.refresh_fc_masks();

    open_at(&mut a, QuickRow::FcMask);
    assert_eq!(a.fc_mask_label(), "无");
    assert_eq!(a.quick_value(QuickRow::FcMask), Some(QuickValue::Off));
    press(&mut a, Btn::Right);
    assert_eq!(a.fc_mask_label(), "FC01.png");
    assert_eq!(
        read_slot_state(d.path()).fc_mask.as_deref(),
        Some("FC01.png")
    );
    assert_eq!(a.quick_value(QuickRow::FcMask), Some(QuickValue::On));
    press(&mut a, Btn::Right);
    assert_eq!(a.fc_mask_label(), "FC02.png");
    assert_eq!(
        read_slot_state(d.path()).fc_mask.as_deref(),
        Some("FC02.png")
    );
    press(&mut a, Btn::Right);
    assert_eq!(a.fc_mask_label(), "无");
    assert_eq!(read_slot_state(d.path()).fc_mask, None);
    // Left walks the same ring backwards.
    press(&mut a, Btn::Left);
    assert_eq!(a.fc_mask_label(), "FC02.png");
    assert_eq!(
        read_slot_state(d.path()).fc_mask.as_deref(),
        Some("FC02.png")
    );
}