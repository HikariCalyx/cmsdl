//! Minimal localization for the GUI patcher status strings.
//!
//! The `.ftl` files use printf-style `%s` placeholders (not real Fluent
//! syntax), so this is a tiny loader: it parses `key = value` lines from the
//! embedded catalogs and substitutes `%s` placeholders in order.

use std::collections::HashMap;
use std::sync::OnceLock;

const ZH_CN: &str = include_str!("locales/zh-CN.ftl");
const EN: &str = include_str!("locales/en.ftl");

/// Parsed catalog for the active language.
static CATALOG: OnceLock<HashMap<String, String>> = OnceLock::new();

/// Parse a `.ftl`-style catalog (`key = value` per line, `#` comments).
fn parse(src: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in src.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Return `true` when the user's preferred UI language is a Chinese variant.
#[cfg(windows)]
fn prefers_chinese() -> bool {
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // Primary language ID is the low 10 bits; LANG_CHINESE == 0x04.
    let langid = unsafe { GetUserDefaultUILanguage() };
    (langid & 0x3FF) == 0x04
}

#[cfg(not(windows))]
fn prefers_chinese() -> bool {
    std::env::var("LANG")
        .map(|l| l.to_ascii_lowercase().contains("zh"))
        .unwrap_or(false)
}

fn catalog() -> &'static HashMap<String, String> {
    CATALOG.get_or_init(|| {
        let src = if prefers_chinese() { ZH_CN } else { EN };
        parse(src)
    })
}

/// Look up `key`, substituting each `%s` placeholder with the next `args`
/// entry in order. Missing keys fall back to the key itself.
pub fn tr(key: &str, args: &[&str]) -> String {
    let template = catalog().get(key).cloned().unwrap_or_else(|| key.to_string());
    let mut out = String::with_capacity(template.len());
    let mut arg_iter = args.iter();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && chars.peek() == Some(&'s') {
            chars.next(); // consume 's'
            if let Some(a) = arg_iter.next() {
                out.push_str(a);
            }
        } else {
            out.push(c);
        }
    }
    out
}
