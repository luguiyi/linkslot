mod art;
pub use art::{contain, cover, decode, decode_bytes};
mod backdrop;
mod barcode;
mod battery;
mod board;
mod cart;
mod clock;
pub mod conflict;
pub mod collection_style;
mod draw;
mod footer;
pub mod guide;
mod hud;
mod icon;
pub mod keyboard;
mod link_art;
pub mod link_dialog;
pub mod mod_choice;
pub mod mods_menu;
pub mod navigation;
pub mod notice;
pub mod network;
mod net_slot;
pub use net_slot::{module_label_face, net_card_face, net_card_label_face, net_module_face, net_slot_face, net_slot_face_with_sticker, NetSlotSticker};
mod plate;
mod polaroids;
mod power_menu;
mod quick_menu;
mod rebuild;
mod refusal;
pub mod search;
mod shelf;
mod shell;
mod silhouette;
mod slot_chrome;
mod status_bar;
mod sticker;
pub mod sync_menu;
pub mod sync_overlay;
pub mod switch_style;
pub mod text;
pub use text::install_font;
mod theme;
mod toast;

pub use backdrop::{
    bezel_face, collection_overlay_face, collection_plate_face, draw_backdrop,
    favorite_stars_face, shelf_backdrop_face, slot_plate_from_art, slot_plate_from_art_inset, wallpaper_face, CollectionKind, SLOT_PLATE_H,
    SLOT_PLATE_W,
};
pub use barcode::{code39, CODE39_NARROW, CODE39_WIDE};
pub use battery::{draw_gauge, gauge_width, BOLT_PX, GAUGE_H, GAUGE_W, WALL};
pub use board::{
    board_at, board_face, board_from, board_from_for, board_size, board_units, board_zoom, chip_face, chip_face_for, chip_shadow_face, core_chip_scale, grown, lid_at,
    lid_from, lid_from_for, lift_of, network_chip_face, network_chip_face_scaled, network_chip_face_scaled_cover,
    network_chip_face_scaled_marked, network_chip_face_scaled_unlit, network_spot, on_board, on_board_for, padded, rom_marking, rom_marking_face, shelf_cart, shelf_cart_at,
    NETWORK_H, NETWORK_U, NETWORK_V, NETWORK_W,
    chip_along, multi_core_slot, single_core_spot, slide_of, slide_up_for, SFC_SLOT_PITCH, socket_face, Placed, BOARD_H, BOARD_W, BOARD_X, BOARD_Y, CAVITIES,
    CHIP_H, CHIP_TIP, CHIP_U, CHIP_V, CHIP_W, HOP_LIFT, LID_TURN, ROM_H, ROM_W, ROM_X, ROM_Y,
    SHADOW_H, SHADOW_W, SLIDE_SHARE, SLIDE_UP, SOCKET_H, SOCKET_U, SOCKET_V, SOCKET_W, TURN_PAD,
};
pub use cart::{
    cart_art_box, cart_box, cart_face, cart_face_with, cart_shadow, clean_label, fc_label_panel,
    gb_cart_shadow, gb_label_panel, gb_shell_of, label_colour, label_panel, label_size, label_tags,
    label_text, labelled_cart_face, labelled_cart_face_with_sticker, labelled_cart_shadow,
    sync_cart_face_with_sticker, SYNC_SHELL, front_size, fit_front,
    seated_box, CartFace, CartSticker, CART_H, CART_W,
    FC_BASE_H, FC_BASE_W, FC_CART_H, FC_CART_W, FC_LABEL_H, FC_LABEL_W, FC_LABEL_X, FC_LABEL_Y,
    GB_CART_H, GB_CART_W,
    GB_LABEL_H, GB_LABEL_W, GB_LABEL_X, GB_LABEL_Y, LABEL_H, LABEL_W, LABEL_X, LABEL_Y,
};
pub use clock::{clock_label, date_time_text, hhmm, set_clock_hint_face, ClockPicker, Field};
pub use conflict::{
    conflict_face, ConflictDirection, ConflictPage, ConflictUnitView, ConflictView, SAV_ACCENT,
    STATE_ACCENT,
};
pub use draw::{Draw, TexId, OUT_H, OUT_W};
pub use footer::Printed;
pub use hud::{
    badge_at, ff_badge, FfState, Hud, HudKind, LinkBadge, Millis, ALARM_BAND_H, HUD_ICON_PX,
    HUD_INK, HUD_MS, LINK_HOST_INK, LINK_JOIN_INK, PLATE_H,
};
pub use icon::{badge_face, icon_box, icon_face, warm_fonts, Badge, Icon};
pub use link_art::{
    link_art, LinkArt, ADAPTER_BASE_X, ADAPTER_BASE_Y, ADAPTER_H, ADAPTER_W, ARCS, ARROW_H,
    ARROW_LEFT_X, ARROW_RIGHT_X, ARROW_W, ARROW_Y, CLICKS_H, CLICKS_W, CLICKS_X, CLICKS_Y, PLUG_H,
    PLUG_TIP_X, PLUG_W, PORT_H, PORT_W, PORT_Y,
};
pub use plate::{
    arrows_hint_face, arrows_hint_width, cap_width, centred_hints, hint_face, hint_face_ink, hint_quad, hint_row,
    hint_width, large_word_face, slot_name_face, title_face, vertical_arrows_hint_face,
    vertical_arrows_hint_width, word_face, word_face_ink, word_width, Hint, UndoFace, SLOT_NAME_H,
    ARROW_GAP, CAP,
    CAP_GAP,
    HINT_EDGE, HINT_GAP, HINT_H, LEGEND_GAP, TITLE_H, TITLE_W,
};
pub use polaroids::{photo_face, PhotoFace, Polaroids, DOT, PHOTO_H, PHOTO_W};
pub use power_menu::{
    menu_face, power_card_face, power_icon_face, power_label_face, power_title_face, PowerChoice,
    MENU_PAD, POWER_CARD_H, POWER_CARD_W,
};
pub use quick_menu::{
    quick_caret_face, quick_caret_face_state, quick_icon_face, quick_label_face,
    quick_label_face_state, quick_legend_faces, quick_title_face, quick_value_face, QuickMenu,
    QuickMenuFaces, QuickRow, QuickValue, QUICK_CARD_H, QUICK_CARD_W, QUICK_EDGE, QUICK_PITCH,
    QUICK_SPLIT, QUICK_TOP, row_top,
};
pub use rebuild::{rebuild_count_face, rebuild_title_face, RebuildScreen};
pub use refusal::Refusal;
/// Re-exported so callers of the quick-menu builders and the integration tests
/// can name the language without depending on `slot-i18n` themselves.
pub use slot_i18n::Language;
pub use search::{candidate_bar, results_face, CANDIDATE_POS};
pub use shelf::{foot_y, playtime_plate, rest_y, set_shelf_slack, Shelf};
pub use shell::{
    gb_table_shells, gba_shell_for, lookup_order_is_exact_then_family_then_default, shell_for,
    shell_presets, table_keys, Finish, Shell, DEFAULT_SHELL, DMG_SHELL, DUAL_MODE_SHELL,
    GB_CLEAR_SHELL,
};
pub use silhouette::{gb_silhouette, silhouette, GbShell};
pub use slot_chrome::{
    draw_empty_slot, draw_empty_slot_with, draw_slot_back_layer, draw_slot_front_layer, draw_slot_name, draw_slot_plate, ease, edge, housing,
    opening, recess, SlotChrome, ALERT_PX, LIP_H, MOUTH_H, MOUTH_W,
};
pub use theme::{current as theme, set_theme};
pub use status_bar::{draw_shelf_legend, wifi_face, StatusBar, STATUS_H, STATUS_MARGIN, WIFI_PX};
pub use sticker::{
    about_face, about_lines, about_thanks_face, draw_about, thanks_lines, AboutFields, AboutPage,
    ABOUT_H, ABOUT_W, AUTHOR, CONTRIBUTOR, CRAZY_SIX, FREE_USE, ORIGINAL_AUTHOR, ORIGINAL_LICENSE,
    ORIGINAL_URL, ORIGINAL_VERSION, PROJECT_VERSION, TEST_WARNING, THANKS,
};
pub use sync_menu::{draw_sync_menu, sync_menu_face, SyncMenuRow, SyncMenuView};
pub use sync_overlay::{arrow_face, cloud_face, line_face, Arrow};
pub use toast::{toast_box, toast_face, toast_rect, Toast};
