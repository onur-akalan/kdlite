//! Translations. The source language is English; other languages live in
//! `lang/<code>.json` (English text -> translation) and are embedded at build
//! time. The language follows the system locale unless `language` is set in
//! the config file.

use std::collections::HashMap;
use std::sync::OnceLock;

/// (language code, catalog JSON). To add a language, add a file and a line.
const CATALOGS: &[(&str, &str)] = &[("tr", include_str!("../lang/tr.json"))];

struct Active {
    code: String,
    json: &'static str,
    map: HashMap<String, String>,
}

static ACTIVE: OnceLock<Active> = OnceLock::new();

fn system_code() -> String {
    ["LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.is_empty() && v != "C" && v != "POSIX")
        .map(|v| v.chars().take_while(|c| c.is_ascii_alphabetic()).collect::<String>().to_lowercase())
        .unwrap_or_default()
}

fn pick(pref: Option<&str>) -> Active {
    let code = match pref.map(|p| p.trim().to_lowercase()) {
        Some(p) if !p.is_empty() && p != "auto" => p,
        _ => system_code(),
    };
    match CATALOGS.iter().find(|(c, _)| *c == code) {
        Some((c, json)) => Active {
            code: c.to_string(),
            json,
            map: serde_json::from_str(json).unwrap_or_default(),
        },
        None => Active {
            code: "en".into(),
            json: "{}",
            map: HashMap::new(),
        },
    }
}

/// Selects the language. `pref` is the `language` setting ("auto", "en", "tr"...).
/// Only the first call has an effect.
pub fn init(pref: Option<&str>) {
    let _ = ACTIVE.set(pick(pref));
}

fn active() -> &'static Active {
    ACTIVE.get_or_init(|| pick(None))
}

/// Active language code ("en", "tr"...).
pub fn lang() -> &'static str {
    &active().code
}

/// The active catalog as JSON (embedded into the window).
pub fn catalog_json() -> &'static str {
    active().json
}

/// Language codes that have a catalog, besides English.
pub fn available() -> Vec<&'static str> {
    CATALOGS.iter().map(|(c, _)| *c).collect()
}

/// Translates an English source string.
pub fn t(s: &str) -> String {
    active().map.get(s).cloned().unwrap_or_else(|| s.to_string())
}

/// Fills `{}` placeholders in order.
pub fn fill(template: String, args: &[String]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template.as_str();
    for a in args {
        match rest.find("{}") {
            Some(i) => {
                out.push_str(&rest[..i]);
                out.push_str(a);
                rest = &rest[i + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// `tr!("Text")` or `tr!("Text {} and {}", a, b)`: translated, then filled.
#[macro_export]
macro_rules! tr {
    ($s:literal) => {
        $crate::i18n::t($s)
    };
    ($s:literal, $($a:expr),+ $(,)?) => {
        $crate::i18n::fill($crate::i18n::t($s), &[$($a.to_string()),+])
    };
}
