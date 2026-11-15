//! `GET /api/i18n`: the browser's copy of the device's translation tables.
//!
//! The Web page asks once, caches the dictionary and re-renders from it.  Both
//! sides read `linkSlot/Config/language.txt`, so the file stays the single source of
//! truth: changing the language on the device and reloading the page, or
//! changing it here and watching the device follow, are the same operation.
//!
//! The dictionary is the *whole* table for one language, not a per-page slice:
//! the page has tabs that lazily build their DOM, and a second round trip per
//! tab would be a visible stall on the device's single HTTP thread.

use serde_json::{json, Map, Value};

use slot_i18n::Language;

use crate::web::http::{self, Request, Response};
use crate::web::Shared;

/// `GET /api/i18n` — `{ "language": "en-US", "strings": { ... } }`.
///
/// `?lang=` is accepted so a page can preview a language other than the saved
/// one; without it the answer follows `linkSlot/Config/language.txt`.
pub fn info(shared: &Shared, request: &Request) -> Response {
    let saved = slot_store::read_language(&shared.root);
    let wanted = match http::query_param(request, "lang") {
        Ok(Some(raw)) => match Language::parse(&raw) {
            Some(language) => language,
            None => {
                // The request's own language is unusable, so the error speaks
                // the language the device is set to.
                let message = slot_i18n::t_args(
                    saved,
                    "web_api.error.language_unknown",
                    &[("code", raw.as_str())],
                );
                return Response::error(400, message);
            }
        },
        Ok(None) => saved,
        Err(error) => return Response::error(error.status, error.message),
    };
    Response::json(
        200,
        json!({
            "language": wanted.code(),
            "strings": dictionary(wanted),
        }),
    )
}

/// The language table as a JSON object, key-sorted for a stable response.
fn dictionary(language: Language) -> Value {
    let mut map = Map::new();
    for (key, value) in slot_i18n::entries(language) {
        map.insert(key.to_string(), Value::String(value.to_string()));
    }
    Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dictionary_is_sorted_and_matches_the_tables() {
        let value = dictionary(Language::EnUs);
        let map = value.as_object().expect("an object");
        assert_eq!(map.len(), slot_i18n::len(Language::EnUs));
        let keys: Vec<&String> = map.keys().collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "the Web dictionary must be key-sorted");
        assert_eq!(
            map.get("device.quick.label.language").and_then(Value::as_str),
            Some("Language")
        );
    }
}
