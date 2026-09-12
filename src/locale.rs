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

/// Map a `ureq` transport failure kind to a localized message.
fn transport_error(kind: ureq::ErrorKind) -> String {
    match kind {
        ureq::ErrorKind::Dns => tr("gui-error-network-dns", &[]),
        ureq::ErrorKind::ConnectionFailed => tr("gui-error-network-connect", &[]),
        ureq::ErrorKind::ProxyConnect
        | ureq::ErrorKind::ProxyUnauthorized
        | ureq::ErrorKind::InvalidProxyUrl => tr("gui-error-proxy", &[]),
        _ => tr("gui-error-network", &[]),
    }
}

/// Turn an operation failure into a short, fully localized sentence suitable
/// for the GUI status line.
///
/// Only the error *category* is shown to the user; the caller is expected to
/// write the raw error (`{e:#}`) to the log file so the technical detail is
/// preserved. Classification walks the anyhow cause chain for known error
/// types (ureq transport/status codes, `std::io::Error`) and otherwise falls
/// back to matching well-known message patterns, ending with a generic
/// "unexpected error" message.
pub fn tr_error(err: &anyhow::Error) -> String {
    // Flattened cause chain, lowercased, used by the pattern fallbacks.
    let mut chain_text = String::new();
    for cause in err.chain() {
        if !chain_text.is_empty() {
            chain_text.push_str(": ");
        }
        chain_text.push_str(&cause.to_string());
    }
    let lower = chain_text.to_ascii_lowercase();

    // 1) Typed HTTP errors from ureq (transport failures + non-2xx status).
    for cause in err.chain() {
        if let Some(ue) = cause.downcast_ref::<ureq::Error>() {
            return match ue {
                ureq::Error::Status(code, _) => tr("gui-error-server", &[&code.to_string()]),
                ureq::Error::Transport(t) => transport_error(t.kind()),
            };
        }
        if let Some(t) = cause.downcast_ref::<ureq::Transport>() {
            return transport_error(t.kind());
        }
    }

    // 2) Typed I/O errors (file system and socket-level failures).
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind as K;
            return match io.kind() {
                K::PermissionDenied => tr("gui-error-permission", &[]),
                K::NotFound => tr("gui-error-not-found", &[]),
                // Socket-level failures surface as plain I/O errors when a
                // response body is being read.
                K::ConnectionRefused
                | K::ConnectionReset
                | K::ConnectionAborted
                | K::NotConnected
                | K::AddrInUse
                | K::AddrNotAvailable
                | K::NetworkDown
                | K::NetworkUnreachable
                | K::HostUnreachable
                | K::TimedOut
                | K::BrokenPipe => tr("gui-error-network", &[]),
                // Distinguish reads from writes by the surrounding context.
                _ if lower.contains("failed to read")
                    || lower.contains("failed to open")
                    || lower.contains("failed to seek") => tr("gui-error-disk-read", &[]),
                _ => tr("gui-error-disk-write", &[]),
            };
        }
    }

    // 3) Well-known message patterns.
    if lower.contains("checksum mismatch")
        || lower.contains("sha-256 mismatch")
        || lower.contains("size mismatch")
        || lower.contains("invalid zip")
        || lower.contains("failed to decompress")
    {
        return tr("gui-error-verification", &[]);
    }
    if lower.contains("download stalled") || lower.contains("stalled after") {
        return tr("gui-error-network", &[]);
    }
    if (lower.contains("not a ") && lower.contains("client directory"))
        || lower.contains("no client data")
    {
        return tr("gui-error-invalid-client", &[]);
    }
    if lower.contains("invalid version number") {
        return tr("gui-error-invalid-version", &[]);
    }
    if lower.contains("no patches are published")
        || lower.contains("patch not found")
        || lower.contains("does not publish patch metadata")
    {
        return tr("gui-error-no-patch", &[]);
    }

    // 4) Fallback.
    tr("gui-error-unknown", &[])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every key referenced by [`tr_error`] must exist in all three catalogs;
    /// a missing key would show the raw key (or an empty message) to the user.
    #[test]
    fn error_keys_present_in_all_catalogs() {
        const KEYS: [&str; 14] = [
            "gui-error-network",
            "gui-error-network-dns",
            "gui-error-network-connect",
            "gui-error-proxy",
            "gui-error-server",
            "gui-error-permission",
            "gui-error-disk-write",
            "gui-error-disk-read",
            "gui-error-not-found",
            "gui-error-verification",
            "gui-error-invalid-client",
            "gui-error-invalid-version",
            "gui-error-no-patch",
            "gui-error-unknown",
        ];
        for (name, src) in [("en", EN), ("zh-CN", ZH_CN), ("zh-TW", ZH_TW)] {
            let cat = parse(src);
            for key in KEYS {
                assert!(cat.contains_key(key), "{name} is missing '{key}'");
            }
        }
    }
}
