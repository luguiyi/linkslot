//! The single-page management UI, embedded into the `slot` binary at compile
//! time.  No CDN, no external font, no runtime asset directory: the device only
//! needs the executable.

use crate::web::http::Response;

pub const INDEX_HTML: &str = include_str!("static/index.html");
pub const APP_CSS: &str = include_str!("static/app.css");
pub const APP_JS: &str = include_str!("static/app.js");
pub const ICON_SVG: &str = include_str!("static/icon.svg");

/// The browser-play page and its offline emulator runtime.
pub const PLAY_HTML: &str = include_str!("play_page/play.html");
pub const PLAY_CSS: &str = include_str!("play_page/play.css");
pub const PLAY_JS: &str = include_str!("play_page/play.js");
pub const PLAY_LOADER_JS: &str = include_str!("play_data/loader.js");
pub const PLAY_EMULATOR_JS: &str = include_str!("play_data/emulator.min.js");
pub const PLAY_EMULATOR_CSS: &str = include_str!("play_data/emulator.min.css");
pub const PLAY_VERSION_JSON: &str = include_str!("play_data/version.json");
pub const PLAY_LOCALIZATION_ZH: &str = include_str!("play_data/localization/zh-CN.json");
pub const PLAY_EXTRACT_7Z: &str = include_str!("play_data/compression/extract7z.js");
pub const PLAY_EXTRACT_ZIP: &str = include_str!("play_data/compression/extractzip.js");
pub const PLAY_MGBA_DATA: &[u8] = include_bytes!("play_data/cores/mgba-legacy-wasm.data");
pub const PLAY_GAMBATTE_DATA: &[u8] = include_bytes!("play_data/cores/gambatte-legacy-wasm.data");
pub const PLAY_FCEUMM_DATA: &[u8] = include_bytes!("play_data/cores/fceumm-legacy-wasm.data");

const CACHE_CONTROL: &str = "no-store";
/// The core blob is big and versioned with the firmware; let the browser keep
/// it for a day so a second visit does not re-download a megabyte.
const CORE_CACHE_CONTROL: &str = "private, max-age=86400";

pub fn route(path: &str) -> Response {
    let response = match path {
        "/" | "/index.html" => text("text/html; charset=utf-8", INDEX_HTML),
        "/app.css" => text("text/css; charset=utf-8", APP_CSS),
        "/app.js" => text("application/javascript; charset=utf-8", APP_JS),
        "/icon.svg" | "/favicon.svg" | "/favicon.ico" => text("image/svg+xml", ICON_SVG),
        "/play" | "/play/" | "/play/index.html" => text("text/html; charset=utf-8", PLAY_HTML),
        "/play/app.css" => text("text/css; charset=utf-8", PLAY_CSS),
        "/play/app.js" => text("application/javascript; charset=utf-8", PLAY_JS),
        "/play/data/loader.js" => text("application/javascript; charset=utf-8", PLAY_LOADER_JS),
        "/play/data/emulator.min.js" => {
            text("application/javascript; charset=utf-8", PLAY_EMULATOR_JS)
        }
        "/play/data/emulator.min.css" => text("text/css; charset=utf-8", PLAY_EMULATOR_CSS),
        "/play/data/version.json" => {
            text("application/json; charset=utf-8", PLAY_VERSION_JSON)
        }
        "/play/data/localization/zh-CN.json" => {
            text("application/json; charset=utf-8", PLAY_LOCALIZATION_ZH)
        }
        "/play/data/compression/extract7z.js" => {
            text("application/javascript; charset=utf-8", PLAY_EXTRACT_7Z)
        }
        "/play/data/compression/extractzip.js" => {
            text("application/javascript; charset=utf-8", PLAY_EXTRACT_ZIP)
        }
        "/play/data/cores/mgba-legacy-wasm.data" => {
            return Response::new(200, "application/octet-stream", PLAY_MGBA_DATA.to_vec())
                .with_header("Cache-Control", CORE_CACHE_CONTROL);
        }
        "/play/data/cores/gambatte-legacy-wasm.data" => {
            return Response::new(200, "application/octet-stream", PLAY_GAMBATTE_DATA.to_vec())
                .with_header("Cache-Control", CORE_CACHE_CONTROL);
        }
        "/play/data/cores/fceumm-legacy-wasm.data" => {
            return Response::new(200, "application/octet-stream", PLAY_FCEUMM_DATA.to_vec())
                .with_header("Cache-Control", CORE_CACHE_CONTROL);
        }
        _ => Response::text(404, "text/plain; charset=utf-8", "页面不存在"),
    };
    response.with_header("Cache-Control", CACHE_CONTROL)
}

fn text(content_type: &'static str, body: &'static str) -> Response {
    Response::new(200, content_type, body.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page has to be able to translate itself: a dictionary loader, the
    /// four tagging attributes and the function that applies them.
    #[test]
    fn the_page_carries_its_own_i18n_layer() {
        for needle in [
            "function loadI18n(",
            "function applyI18n(",
            "function t(key, params)",
            "data-i18n=",
            "data-i18n-placeholder=",
            "data-i18n-title=",
            "data-i18n-aria=",
            "/api/i18n",
        ] {
            assert!(
                APP_JS.contains(needle) || INDEX_HTML.contains(needle),
                "the i18n layer is missing {needle}"
            );
        }
        // Translations are text, never markup.
        assert!(!APP_JS.contains("innerHTML = t("), "a translation is injected as HTML");
        // The long developer reference has a hand-written English sibling; the
        // Chinese card is hidden in English instead of leaving a half-Chinese page.
        assert!(INDEX_HTML.contains("id=\"modsApiCardEn\""));
        assert!(APP_JS.contains("modsApiCardEn"));
        // The Chinese page must survive a failed load.
        assert!(INDEX_HTML.contains(">概览<"), "the Chinese fallback markup is gone");
        // The cover editor must not paint a black matte over the uncovered
        // sticker area; the cartridge colour shows through instead.
        assert!(!APP_JS.contains("ctx.fillStyle = '#141424'"));
        assert!(APP_JS.contains("canvas.style.backgroundColor"));
    }

    /// Every `data-i18n` / `t("web.…")` key must exist in both tables, or the
    /// page shows a key name to the user.
    #[test]
    fn every_tagged_key_exists_in_both_languages() {
        use slot_i18n::Language;
        let mut keys: Vec<&str> = Vec::new();
        for attr in ["data-i18n", "data-i18n-placeholder", "data-i18n-title", "data-i18n-aria"] {
            let needle = format!("{attr}=\"");
            let mut rest = INDEX_HTML;
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                let Some(end) = rest.find('"') else { break };
                keys.push(&rest[..end]);
            }
        }
        assert!(keys.len() > 100, "only {} tagged nodes", keys.len());
        let mut missing = Vec::new();
        for key in keys {
            for lang in Language::ALL {
                if slot_i18n::t_opt(lang, key).is_none() {
                    missing.push(format!("{key} ({})", lang.code()));
                }
            }
        }
        assert!(missing.is_empty(), "missing translations: {missing:?}");
    }

    /// Keys built in `app.js` must resolve too: a missing entry would paint the
    /// raw `web.…` key into a toast, button or table cell.  Scan every quoted
    /// `web.` literal so the explicit `labelKey` fields for mod categories are
    /// covered as well.
    #[test]
    fn every_dynamic_key_exists_in_both_languages() {
        use slot_i18n::Language;
        let mut keys: Vec<&str> = Vec::new();
        let mut rest = APP_JS;
        while let Some(at) = rest.find("'web.") {
            rest = &rest[at + 1..];
            let Some(end) = rest.find('\'') else { break };
            let key = &rest[..end];
            let valid = key
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_');
            if valid && !keys.contains(&key) {
                keys.push(key);
            }
        }
        assert!(keys.len() > 350, "only {} dynamic keys", keys.len());
        let mut missing = Vec::new();
        for key in keys {
            // These are runtime prefixes (for example `shape_` + `rect`);
            // the concrete suffixed keys are checked by the HTML test.
            if key.ends_with('_') {
                continue;
            }
            for lang in Language::ALL {
                if slot_i18n::t_opt(lang, key).is_none() {
                    missing.push(format!("{key} ({})", lang.code()));
                }
            }
        }
        assert!(missing.is_empty(), "missing dynamic translations: {missing:?}");
    }

    #[test]
    fn assets_are_embedded() {
        assert!(INDEX_HTML.len() > 1000, "index.html looks empty");
        assert!(APP_CSS.len() > 2000, "app.css looks empty");
        assert!(APP_JS.len() > 2000, "app.js looks empty");
        assert!(ICON_SVG.contains("<svg"), "icon.svg is not SVG");
    }

    /// Adding a game is a platform choice in the library page. Every machine
    /// the shelf scans must be selectable there, and the filter must offer it
    /// too, or a freshly uploaded SFC game would be invisible.
    #[test]
    fn the_library_page_offers_every_platform() {
        for platform in slot_store::Platform::ALL {
            let name = platform.dir_name();
            assert!(
                INDEX_HTML.contains(&format!("data-platform=\"{name}\"")),
                "the library filter is missing {name}"
            );
            assert!(
                INDEX_HTML.contains(&format!("<option value=\"{name}\"")),
                "the add-game dialog is missing {name}"
            );
            assert!(
                APP_JS.contains(&format!("{name}:")),
                "app.js has no add/extension entry for {name}"
            );
        }
    }

    #[test]
    fn the_play_page_is_embedded_with_its_offline_runtime() {
        assert!(PLAY_HTML.contains("id=\"game\""), "play.html has no stage");
        assert!(PLAY_HTML.contains("/play/app.js"));
        assert!(PLAY_CSS.contains(".stage"));
        assert!(PLAY_JS.contains("/api/play/rom"));
        assert!(PLAY_JS.contains("/api/play/save"));
        assert!(PLAY_JS.contains("/play/data/loader.js"));
        // The battery save is synced silently: no panel, no manual button.
        assert!(!PLAY_HTML.contains("立即写回"), "the save panel came back");
        assert!(!PLAY_HTML.contains("write-back"), "the save button came back");
        assert!(!PLAY_HTML.contains("data-act=\"save\""), "the pause save button came back");
        assert!(PLAY_JS.contains("SAVE_SYNC_MS"), "the silent sweep is gone");
        // The device's stamp is an opaque digest; Number() would turn it into
        // NaN and every write-back into a 409.
        assert!(PLAY_JS.contains("state.saveStamp = response.headers.get('X-Save-Stamp')"));
        assert!(!PLAY_JS.contains("Number(response.headers.get('X-Save-Stamp')"));
        // Two-player Famicom: a second pad mapping and the toggle that swaps
        // the running emulator over to it.
        assert!(PLAY_JS.contains("DEFAULT_P2"));
        assert!(PLAY_JS.contains("TWO_PLAYER_P1"));
        assert!(PLAY_JS.contains("applyTwoPlayer"));
        assert!(PLAY_JS.contains("EJS_defaultControls"));
        assert!(PLAY_JS.contains("value: 'j'"), "1P J is not mapped");
        assert!(PLAY_JS.contains("value: '1'"), "2P 1 is not mapped");
        assert!(PLAY_HTML.contains("P1："), "the 1P WASD/JKL hint is missing");
        assert!(PLAY_HTML.contains("数字 1–6"), "the 2P number hint is missing");
        assert!(PLAY_HTML.contains("id=\"two-player\""));
        assert!(PLAY_HTML.contains("双人同屏（FC）"));
        // Two gamepads: the first is player 1, the second only drives player 2
        // while the two-player mapping exists.
        assert!(PLAY_JS.contains("gamepadconnected"));
        assert!(PLAY_JS.contains("connectedPads"));
        assert!(PLAY_HTML.contains("id=\"pad-state\""));
        assert!(PLAY_HTML.contains("手柄 1 → 1P"));
        // The detail dialog shows the intro text, not just "已上传".
        assert!(INDEX_HTML.contains("id=\"romIntroBody\""));
        assert!(APP_JS.contains("function loadRomIntro"));
        assert!(APP_CSS.contains(".rom-intro-body"));
        // A lost keyup must never leave a direction held down.
        assert!(PLAY_JS.contains("releaseAllInputs"));
        assert!(PLAY_JS.contains("forwardKeyToGame"));
        assert!(PLAY_JS.contains("pointercancel"));
        // The bundle must stay self-contained: no CDN fallbacks.
        for needle in ["emulatorjs cdn", "http://", "https://"] {
            assert!(
                !PLAY_JS.contains(needle),
                "play.js references {needle}"
            );
        }
        assert!(PLAY_EMULATOR_JS.len() > 100_000, "the emulator bundle is missing");
        assert!(PLAY_MGBA_DATA.len() > 500_000, "the mGBA core data is missing");
        assert!(PLAY_GAMBATTE_DATA.len() > 500_000, "the Gambatte core data is missing");
        assert!(PLAY_FCEUMM_DATA.len() > 500_000, "the FCEUmm core data is missing");
        // Core choice follows the platform in Games/<机种>/…
        assert!(PLAY_JS.contains("'GBA' ? 'mgba'"));
        assert!(PLAY_JS.contains("FC"));
        assert!(PLAY_JS.contains("'nes'"));
    }

    #[test]
    fn the_play_page_loads_the_same_i18n_dictionary() {
        for needle in ["function loadI18n(", "function applyI18n(", "'/api/i18n'"] {
            assert!(PLAY_JS.contains(needle), "play.js is missing {needle}");
        }
        let mut keys: Vec<&str> = Vec::new();
        for attr in ["data-i18n", "data-i18n-title"] {
            let needle = format!("{attr}=\"");
            let mut rest = PLAY_HTML;
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                let Some(end) = rest.find('"') else { break };
                keys.push(&rest[..end]);
            }
        }
        let mut rest = PLAY_JS;
        while let Some(at) = rest.find("t('play.") {
            rest = &rest[at + 3..];
            let Some(end) = rest.find('\'') else { break };
            keys.push(&rest[..end]);
        }
        assert!(keys.len() > 20, "only {} play keys", keys.len());
        for key in keys {
            for lang in slot_i18n::Language::ALL {
                assert!(
                    slot_i18n::t_opt(lang, key).is_some(),
                    "missing {key} for {}",
                    lang.code()
                );
            }
        }
    }

    #[test]
    fn play_urls_are_served_from_the_binary() {
        let page = route("/play");
        assert_eq!(page.status, 200);
        assert_eq!(page.content_type, "text/html; charset=utf-8");
        assert!(String::from_utf8_lossy(&page.body).contains("在浏览器游玩"));

        let core = route("/play/data/cores/mgba-legacy-wasm.data");
        assert_eq!(core.status, 200);
        assert_eq!(core.content_type, "application/octet-stream");
        assert_eq!(core.body.len(), PLAY_MGBA_DATA.len());

        let gb_core = route("/play/data/cores/gambatte-legacy-wasm.data");
        assert_eq!(gb_core.status, 200);
        assert_eq!(gb_core.body.len(), PLAY_GAMBATTE_DATA.len());

        let fc_core = route("/play/data/cores/fceumm-legacy-wasm.data");
        assert_eq!(fc_core.status, 200);
        assert_eq!(fc_core.body.len(), PLAY_FCEUMM_DATA.len());

        let locale = route("/play/data/localization/zh-CN.json");
        assert_eq!(locale.status, 200);

        let missing = route("/play/data/nope.js");
        assert_eq!(missing.status, 404);
    }

    #[test]
    fn the_game_list_is_not_rescanned_for_every_cover() {
        // The cover handler resolves Labels/… directly; a regression would put
        // `find(shared, request)` (a full library scan) back in it.
        let roms = include_str!("roms.rs");
        let cover = roms
            .split("pub(super) fn cover(")
            .nth(1)
            .and_then(|rest| rest.split("pub(super) fn cover_put(").next())
            .expect("the cover handler is missing");
        assert!(
            !cover.contains("find(shared"),
            "cover() scans the whole library again"
        );
        assert!(
            cover.contains("cart_meta::read"),
            "cover() no longer reads the metadata row"
        );
    }

    #[test]
    fn the_game_dialog_offers_browser_play() {
        assert!(INDEX_HTML.contains("id=\"romPlayWeb\""));
        assert!(APP_JS.contains("function playInBrowser"));
        assert!(APP_JS.contains("/play?rom="));
    }

    #[test]
    fn index_references_the_embedded_assets() {
        assert!(INDEX_HTML.contains("href=\"/app.css\""));
        assert!(INDEX_HTML.contains("src=\"/app.js\""));
        assert!(INDEX_HTML.contains("/icon.svg"));
        assert!(INDEX_HTML.contains("lang=\"zh-CN\""));
        assert!(INDEX_HTML.contains("viewport"));
        // No third-party resources: everything must be same-origin.
        for needle in ["//cdn", "googleapis", "unpkg", "jsdelivr", "fonts."] {
            assert!(
                !INDEX_HTML.contains(needle),
                "index.html references {needle}"
            );
        }
        assert!(
            !INDEX_HTML.contains("src=\"http"),
            "index.html loads a remote script"
        );
        assert!(
            !INDEX_HTML.contains("href=\"http"),
            "index.html loads a remote stylesheet"
        );
        // Play-time overview card (task-7) is wired to app.js via this id.
        assert!(INDEX_HTML.contains("id=\"ovPlaytime\""));
        // System upgrade upload (the actual install is device-side only).
        assert!(INDEX_HTML.contains("id=\"upgradeFile\""));
        assert!(INDEX_HTML.contains("id=\"upgradeUpload\""));
        assert!(INDEX_HTML.contains("id=\"upgradeStatus\""));
        // The upload reports progress: a bar the script fills while the bytes
        // go out, then the verify shimmer once they are in.
        assert!(INDEX_HTML.contains("id=\"upgradeProgress\""));
        assert!(INDEX_HTML.contains("id=\"upgradeProgressBar\""));
        // Log export: one button that downloads Logs/ plus its crash records.
        assert!(INDEX_HTML.contains("id=\"exportLogs\""));
        assert!(INDEX_HTML.contains("id=\"exportLogsStatus\""));
        assert!(APP_JS.contains("/api/logs/export"));
        assert!(INDEX_HTML.contains("id=\"upgradeProgressText\""));
        for id in [
            "uploadProgressOverlay",
            "uploadProgressTitle",
            "uploadProgressFile",
            "uploadProgressBar",
            "uploadProgressText",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "index.html is missing {id}"
            );
        }
        assert!(APP_JS.contains("beginUpload"));
        assert!(APP_JS.contains("endUpload"));
        assert!(APP_JS.contains("uploadWithProgress"));
        assert!(APP_CSS.contains(".upload-overlay"));
        assert!(APP_JS.contains("xhr.upload.onprogress"));
        assert!(APP_CSS.contains(".upload-progress-bar"));
        assert!(APP_JS.contains("/api/upgrade"));
        assert!(!APP_JS.contains("/api/upgrade/apply"));

        // ROM library and its two edit dialogs.
        assert!(INDEX_HTML.contains("data-tab=\"roms\""));
        assert!(INDEX_HTML.contains("data-page=\"roms\""));
        assert!(INDEX_HTML.contains("data-tab=\"files\""));
        assert!(INDEX_HTML.contains("data-page=\"files\""));
        for id in [
            "fileBreadcrumb",
            "fileList",
            "fileUpload",
            "fileUploadInput",
            "fileNewFolder",
            "filePreviewDialog",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "index.html is missing {id}"
            );
        }
        assert!(APP_JS.contains("/api/files"));
        assert!(APP_JS.contains("function bindFileBrowser"));
        for id in [
            "romList",
            "romsSummary",
            "refreshRoms",
            "romDialog",
            "romReplaceCover",
            "romReplaceRom",
            "coverDialog",
            "coverCanvas",
            "coverSave",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "index.html is missing {id}"
            );
        }
        assert!(INDEX_HTML.contains("data-tab=\"settings\""));
        assert!(INDEX_HTML.contains("data-page=\"settings\""));
        for id in [
            "webSettingsForm",
            "webUploadMb",
            "saveWebSettings",
            "resetWebSettings",
            "webSettingsStatus",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "index.html is missing {id}"
            );
        }
        assert!(APP_JS.contains("/api/settings"));
        assert!(APP_JS.contains("loadWebSettings"));
        assert!(APP_JS.contains("bindSettings"));
        assert!(APP_JS.contains("useUploadLimit"));
        assert!(INDEX_HTML.contains("id=\"storageList\""));
        assert!(INDEX_HTML.contains("id=\"addStorage\""));
        assert!(INDEX_HTML.contains("id=\"wdName\""));
        assert!(APP_JS.contains("active_storage"));
        assert!(APP_JS.contains("storageList"));
        assert!(APP_JS.contains("/api/sync/config"));
        // Lua script page (mods v1): list, detail editor, compatibility card
        // and read-only API cheat sheet.  The page is the only doc the user has
        // on the handheld, so every control id is asserted here.
        assert!(INDEX_HTML.contains("data-tab=\"mods\""));
        assert!(INDEX_HTML.contains("data-tab=\"mods\" data-i18n=\"web.nav.mods\">mods</a>"));
        assert!(INDEX_HTML.contains("data-page=\"mods\""));
        for id in [
            "modsRuntimeBanner",
            "refreshMods",
            "modImport",
            "modExport",
            "modImportFile",
            "modsSummary",
            "modList",
            "modDetailDialog",
            "modDetailTitle",
            "modDetailId",
            "modDetailVersion",
            "modDetailAuthor",
            "modDetailStatus",
            "openModUi",
            "uninstallMod",
            "modReadme",
            "closeModDetail",
            "modUiDialog",
            "modUiTitle",
            "modUiHint",
            "modUiFrame",
            "modUiStatus",
            "closeModUi",
            "refreshModsLog",
            "modsLog",
            "modsApiCard",
        ] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "index.html is missing {id}"
            );
        }
        assert!(INDEX_HTML.contains("API 速查"));
        assert!(INDEX_HTML.contains("mod 开发文档"));
        assert!(INDEX_HTML.contains("LinkSlotMod"));
        assert!(INDEX_HTML.contains("复制 AI 提示词"));
        for id in ["doc-ai", "mod-ai-prompt", "mod-api-quick", "doc-web", "doc-limits"] {
            assert!(
                INDEX_HTML.contains(&format!("id=\"{id}\"")),
                "mod docs is missing {id}"
            );
        }
        assert!(APP_JS.contains("bindModDocs"));
        assert!(APP_JS.contains("navigator.clipboard"));
        assert!(APP_JS.contains("boot_delay_minutes"));
        assert!(APP_JS.contains("action_id: action.id"));
        assert!(INDEX_HTML.contains("after_boot"));
        assert!(INDEX_HTML.contains("boot_delay_minutes"));
        assert!(INDEX_HTML.contains("sandbox=\"allow-scripts allow-forms\""));
        assert!(INDEX_HTML.contains("X-Slot-Web"));
    }

    #[test]
    fn app_js_uses_the_required_write_header() {
        assert!(APP_JS.contains("X-Slot-Web"));
        assert!(APP_JS.contains("/api/status"));
        assert!(APP_JS.contains("/api/saves"));
        assert!(APP_JS.contains("/api/file"));
        assert!(APP_JS.contains("/api/sync"));
        assert!(APP_JS.contains("/api/network"));
        assert!(APP_JS.contains("/api/roms"));
        assert!(APP_JS.contains("/api/rom/cover"));
        assert!(APP_JS.contains("/api/rom/color"));
        assert!(APP_JS.contains("/api/rom?rom="));
        assert!(APP_JS.contains("function loadRoms"));
        assert!(APP_JS.contains("function saveCover"));
        assert!(APP_JS.contains("function saveRomColor"));
        // Play time (task-7): formatter + both API fields consumed.
        assert!(APP_JS.contains("function fmtDuration"));
        assert!(APP_JS.contains("playtime_seconds"));
        assert!(APP_JS.contains("playtime_total_seconds"));
        assert!(APP_JS.contains("ovPlaytime"));
        assert!(APP_JS.contains("unit-playtime"));
        // Lua script page: every documented endpoint is wired, and the page
        // keeps the same apiWrite/X-Slot-Web write path as the other cards.
        for endpoint in [
            "/api/mods",
            "/api/mods/item",
            "/api/mods/toggle",
            "/api/mods/check",
            "/api/mods/uninstall",
            "/api/mods/action",
            "/api/mods/action/toggle",
            "/api/mods/action/delete",
            "/api/mods/files",
            "/api/mods/file",
        ] {
            assert!(APP_JS.contains(endpoint), "app.js is missing {endpoint}");
        }
        for function in [
            "function loadMods",
            "function deleteSave",
            "function openModDetail",
            "function fillModDetail",
            "function loadModDetailViews",
            "function openModUi",
            "function closeModUi",
            "function handleModUiMessage",
            "function dispatchModUiRequest",
            "function uninstallMod",
            "function exportMods",
            "function importMods",
            "function buildStoreZip",
            "function readZipEntries",
        ] {
            assert!(APP_JS.contains(function), "app.js is missing {function}");
        }
        assert!(APP_JS.contains("data-mod-toggle"));
        assert!(APP_JS.contains("modUiFrame"));
        assert!(APP_JS.contains("/mod-ui/"));
        assert!(APP_JS.contains("modVersionProblem"));
        assert!(APP_JS.contains("modDetailVersion"));
        assert!(APP_JS.contains("files.write"));
        assert!(APP_JS.contains("actions.save"));
        assert!(APP_JS.contains("data-action=\"delete-save\""));
        assert!(APP_JS.contains("/api/saves/delete"));
        assert!(APP_JS.contains("linkSlot.mods.zip"));
        assert!(APP_JS.contains("manifest.json"));
        assert!(APP_JS.contains("runtime_hint"));
        assert!(APP_JS.contains("web.mods.runtime_missing"));
        assert!(APP_JS.contains("locale: i18n.lang"));
        // The new cards reuse the existing table/form styles plus a few
        // mods-specific classes for the compatibility rows and Lua editor.
        assert!(APP_CSS.contains(".mods-toolbar"));
        assert!(APP_CSS.contains(".mod-ui-dialog"));
        assert!(APP_CSS.contains(".mod-error"));
        assert!(APP_CSS.contains(".mod-dialog"));
        assert!(APP_CSS.contains("textarea"));
    }

    #[test]
    fn routes_have_content_types() {
        assert_eq!(route("/").content_type, "text/html; charset=utf-8");
        assert_eq!(route("/app.css").content_type, "text/css; charset=utf-8");
        assert_eq!(
            route("/app.js").content_type,
            "application/javascript; charset=utf-8"
        );
        assert_eq!(route("/icon.svg").content_type, "image/svg+xml");
        assert_eq!(route("/nope").status, 404);
        assert_eq!(route("/").status, 200);
    }
}
