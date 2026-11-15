//! Generic modal used by a manual mod to ask the user to choose one option.
//!
//! The face is intentionally translucent: the shelf stays visible behind a
//! dimmed overlay and the choice card is the only opaque element.  It is not
//! tied to sync units or the cloud, so any `manual` action can use it.

use slot_i18n::Language;

use slot_gfx::{OUT_H, OUT_W};

use crate::keyboard as kb;
use crate::plate::UndoFace;
use crate::switch_style as sw;

const DIALOG_W: f32 = 620.0;
const DIALOG_H: f32 = 380.0;
const ROW_H: f32 = 40.0;
const ROW_GAP: f32 = 5.0;
const OPTION_TOP: f32 = 150.0;

/// Everything the modal prints.  The app/frontend use equality as the
/// texture-cache key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModChoiceView {
    pub title: String,
    pub body: String,
    pub options: Vec<String>,
    pub selected: usize,
}

impl ModChoiceView {
    pub fn selected_index(&self) -> usize {
        if self.options.is_empty() {
            0
        } else {
            self.selected.min(self.options.len() - 1)
        }
    }
}

fn accent() -> [u8; 4] {
    [sw::accent()[0], sw::accent()[1], sw::accent()[2], 0xff]
}

pub fn mod_choice_face(view: &ModChoiceView, lang: Language) -> UndoFace {
    let mut face = kb::blank(OUT_W, OUT_H, [0, 0, 0, 0]);
    kb::fill(
        &mut face,
        0.0,
        0.0,
        OUT_W as f32,
        OUT_H as f32,
        [0, 0, 0, 168],
    );

    let x = ((OUT_W as f32 - DIALOG_W) / 2.0).round();
    let y = ((OUT_H as f32 - DIALOG_H) / 2.0).round();
    kb::fill(
        &mut face,
        x + 8.0,
        y + 10.0,
        DIALOG_W,
        DIALOG_H,
        [0, 0, 0, 96],
    );
    sw::fill_round(
        &mut face,
        x,
        y,
        DIALOG_W,
        DIALOG_H,
        sw::DIALOG_RADIUS,
        sw::surface_border(),
    );
    sw::fill_round(
        &mut face,
        x + 1.0,
        y + 1.0,
        DIALOG_W - 2.0,
        DIALOG_H - 2.0,
        (sw::DIALOG_RADIUS - 1.0).max(0.0),
        sw::surface(),
    );
    kb::fill(&mut face, x + 32.0, y + 26.0, 5.0, 28.0, accent());

    let title = if view.title.is_empty() {
        slot_i18n::t(lang, "device.app.mod_choice.choose")
    } else {
        view.title.as_str()
    };
    kb::draw_left(
        &mut face,
        title,
        26.0,
        14.0,
        DIALOG_W - 96.0,
        x + 48.0,
        y + 18.0,
        44.0,
        sw::ink(),
    );
    kb::draw_wrapped(
        &mut face,
        &view.body,
        16.0,
        11.0,
        x + 48.0,
        y + 78.0,
        DIALOG_W - 96.0,
        66.0,
        3,
        sw::ink_dim(),
    );

    let selected = view.selected_index();
    for (index, label) in view.options.iter().take(4).enumerate() {
        let top = y + OPTION_TOP + index as f32 * (ROW_H + ROW_GAP);
        if index == selected {
            sw::fill_round(
                &mut face,
                x + 40.0,
                top,
                DIALOG_W - 80.0,
                ROW_H,
                8.0,
                sw::selected(),
            );
            kb::draw_left(
                &mut face,
                label,
                18.0,
                12.0,
                DIALOG_W - 128.0,
                x + 62.0,
                top + 8.0,
                30.0,
                sw::ink_selected(),
            );
        } else {
            sw::fill_round(
                &mut face,
                x + 40.0,
                top,
                DIALOG_W - 80.0,
                ROW_H,
                8.0,
                sw::surface_border(),
            );
            kb::draw_left(
                &mut face,
                label,
                18.0,
                12.0,
                DIALOG_W - 128.0,
                x + 62.0,
                top + 8.0,
                30.0,
                sw::ink(),
            );
        }
    }

    if view.options.is_empty() {
        kb::draw_left(
            &mut face,
            slot_i18n::t(lang, "device.app.mod_choice.none"),
            16.0,
            11.0,
            DIALOG_W - 96.0,
            x + 48.0,
            y + OPTION_TOP + 8.0,
            28.0,
            sw::ink_dim(),
        );
    }

    kb::draw_box(
        &mut face,
        slot_i18n::t(lang, "device.app.mod_choice.hint"),
        16.0,
        11.0,
        x + 40.0,
        y + DIALOG_H - 48.0,
        DIALOG_W - 80.0,
        30.0,
        sw::ink_dim(),
    );
    face
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(selected: usize) -> ModChoiceView {
        ModChoiceView {
            title: "存档冲突".to_string(),
            body: "本地和 GBAStation 都修改了电池存档。".to_string(),
            options: vec![
                "用 GBAStation 覆盖本地".to_string(),
                "用本地覆盖 GBAStation".to_string(),
            ],
            selected,
        }
    }

    #[test]
    fn face_is_full_screen_and_selection_changes_pixels() {
        let first = mod_choice_face(&view(0), Language::ZhCn);
        let second = mod_choice_face(&view(1), Language::ZhCn);
        assert_eq!((first.w, first.h), (OUT_W, OUT_H));
        assert_ne!(first.rgba, second.rgba, "selected row did not move");
        assert!(
            first
                .rgba
                .chunks(4)
                .any(|pixel| pixel[0] > 48 && pixel[1] > 48 && pixel[2] > 48),
            "the choice card has no visible pixels"
        );
    }

    #[test]
    fn selected_index_is_clamped() {
        assert_eq!(view(9).selected_index(), 1);
        let empty = ModChoiceView::default();
        assert_eq!(empty.selected_index(), 0);
        let face = mod_choice_face(&empty, Language::ZhCn);
        assert_eq!((face.w, face.h), (OUT_W, OUT_H));
    }
}
