use slot_i18n::Language;

use crate::cart::CartFace;
use crate::plate::UndoFace;
use crate::switch_style;
use crate::text;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PowerChoice {
    Restart,
    PowerOff,
    Repair,
}

impl PowerChoice {
    pub const ALL: [PowerChoice; 3] = [
        PowerChoice::Restart,
        PowerChoice::PowerOff,
        PowerChoice::Repair,
    ];

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn text(self, lang: Language) -> &'static str {
        match self {
            PowerChoice::Restart => slot_i18n::t(lang, "device.app.power.choice.restart"),
            PowerChoice::PowerOff => slot_i18n::t(lang, "device.app.power.choice.power_off"),
            PowerChoice::Repair => slot_i18n::t(lang, "device.app.power.choice.repair"),
        }
    }

    pub fn glyph(self) -> char {
        match self {
            PowerChoice::Restart => '\u{f01e}',
            PowerChoice::PowerOff => '\u{f011}',
            PowerChoice::Repair => '\u{f0ad}',
        }
    }

    pub fn icon_text(self, lang: Language) -> &'static str {
        match self {
            PowerChoice::Restart => slot_i18n::t(lang, "device.app.power.choice.restart_short"),
            PowerChoice::PowerOff => slot_i18n::t(lang, "device.app.power.choice.power_off_short"),
            PowerChoice::Repair => slot_i18n::t(lang, "device.app.power.choice.repair_short"),
        }
    }
}

pub const POWER_CARD_W: u32 = 520;
pub const POWER_CARD_H: u32 = 52;

pub fn power_card_face(selected: bool) -> UndoFace {
    switch_style::card_face(POWER_CARD_W, POWER_CARD_H, selected)
}

pub fn power_label_face(choice: PowerChoice, lit: bool, lang: Language) -> UndoFace {
    let colour = if lit {
        switch_style::ink_selected()
    } else {
        switch_style::ink()
    };
    text_face(choice.text(lang), colour)
}

pub fn power_icon_face(choice: PowerChoice, lit: bool, lang: Language) -> CartFace {
    let colour = if lit {
        switch_style::accent()
    } else {
        switch_style::ink()
    };
    let face = crate::icon::glyph_face(choice.glyph(), 28.0, colour);
    if face.w > 0 {
        face
    } else {
        let fill = if lit {
            [
                switch_style::accent()[0],
                switch_style::accent()[1],
                switch_style::accent()[2],
                255,
            ]
        } else {
            switch_style::surface_border()
        };
        let face = switch_style::tile_face(choice.icon_text(lang), fill, switch_style::ink(), 28);
        CartFace {
            rgba: face.rgba,
            w: face.w,
            h: face.h,
        }
    }
}

pub fn power_title_face(lang: Language) -> UndoFace {
    switch_style::title_face(slot_i18n::t(lang, "device.app.power.title"), 30.0)
}

fn text_face(label: &str, colour: [u8; 3]) -> UndoFace {
    let Some(font) = text::label_font() else {
        return UndoFace {
            rgba: Vec::new(),
            w: 0,
            h: 0,
        };
    };
    let ink = text::line_width(font, label, MENU_PX, 0.0).ceil() as u32;
    let w = ink + 2 * MENU_PAD;
    let mut rgba = vec![0u8; (w * MENU_H * 4) as usize];
    let layout = text::fit(font, label, w as f32, 1, MENU_PX, MENU_MIN_PX);
    text::draw_centred(&mut rgba, w, MENU_H, &layout, colour);
    UndoFace { rgba, w, h: MENU_H }
}

pub(crate) const MENU_PX: f32 = 30.0;
const MENU_MIN_PX: f32 = 18.0;
pub(crate) const MENU_H: u32 = 40;
pub const MENU_PAD: u32 = 18;
pub(crate) const MENU_INK: [u8; 3] = [0xf6, 0xf4, 0xef];

pub fn menu_face(label: &str) -> UndoFace {
    text_face(label, MENU_INK)
}
