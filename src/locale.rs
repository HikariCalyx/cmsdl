//! Minimal localization for the GUI patcher status strings.
//!
//! The `.ftl` files use printf-style `%s` placeholders (not real Fluent
//! syntax), so this is a tiny loader: it parses `key = value` lines from the
//! embedded catalogs and substitutes `%s` placeholders in order.

use std::collections::HashMap;
use std::sync::OnceLock;

const ZH_CN: &str = include_str!("locales/zh-CN.ftl");
const ZH_TW: &str = include_str!("locales/zh-TW.ftl");
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

enum Lang { ZhTw, ZhCn, En }

/// Detect the user's preferred UI language, distinguishing Traditional Chinese
/// variants (zh-TW, zh-HK, zh-MO) from Simplified Chinese (zh-CN/zh-SG).
#[cfg(windows)]
fn detect_lang() -> Lang {
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    let langid = unsafe { GetUserDefaultUILanguage() };
    // Primary language ID is the low 10 bits; LANG_CHINESE == 0x04.
    if (langid & 0x3FF) != 0x04 {
        return Lang::En;
    }
    // Sub-language ID is bits 15-10.
    // SUBLANG_CHINESE_TRADITIONAL (TW) = 0x01
    // SUBLANG_CHINESE_HONGKONG  (HK)  = 0x03
    // SUBLANG_CHINESE_MACAU     (MO)  = 0x05
    match (langid >> 10) & 0x3F {
        0x01 | 0x03 | 0x05 => Lang::ZhTw,
        _ => Lang::ZhCn,
    }
}

#[cfg(not(windows))]
fn detect_lang() -> Lang {
    let lang = std::env::var("LANG")
        .unwrap_or_default()
        .to_ascii_lowercase();
    if !lang.contains("zh") {
        return Lang::En;
    }
    // Match zh_tw, zh_hk, zh_mo (and zh-tw etc.) to Traditional Chinese.
    if lang.contains("tw") || lang.contains("hk") || lang.contains("mo") {
        Lang::ZhTw
    } else {
        Lang::ZhCn
    }
}

fn catalog() -> &'static HashMap<String, String> {
    CATALOG.get_or_init(|| {
        let src = match detect_lang() {
            Lang::ZhTw => ZH_TW,
            Lang::ZhCn => ZH_CN,
            Lang::En => EN,
        };
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
