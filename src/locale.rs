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

/// Map a `ureq` transport failure kind to its message key.
fn transport_error(kind: ureq::ErrorKind) -> &'static str {
    match kind {
        ureq::ErrorKind::Dns => "gui-error-network-dns",
        ureq::ErrorKind::ConnectionFailed => "gui-error-network-connect",
        ureq::ErrorKind::ProxyConnect
        | ureq::ErrorKind::ProxyUnauthorized
        | ureq::ErrorKind::InvalidProxyUrl => "gui-error-proxy",
        _ => "gui-error-network",
    }
}

/// Turn an operation failure into a short, fully localized sentence suitable
/// for the GUI status line.
///
/// Only the error *category* is shown to the user; the caller is expected to
/// write the raw error (`{e:#}`) to the log file so the technical detail is
/// preserved. See [`error_key`] for how the category is chosen.
pub fn tr_error(err: &anyhow::Error) -> String {
    match error_key(err) {
        (key, Some(arg)) => tr(key, &[&arg]),
        (key, None) => tr(key, &[]),
    }
}

/// Whether [`tr_error`] reports `err` as `gui-error-network-connect`, i.e. the
/// server could not be connected to at all.
pub fn is_network_connect_error(err: &anyhow::Error) -> bool {
    error_key(err).0 == "gui-error-network-connect"
}

/// Pick the message key [`tr_error`] shows for `err`, plus the value for its
/// `%s` placeholder (only `gui-error-server` has one: the HTTP status code).
///
/// Classification walks the anyhow cause chain for known error types (ureq
/// transport/status codes, `std::io::Error`) and otherwise falls back to
/// matching well-known message patterns, ending with a generic "unexpected
/// error" message.
fn error_key(err: &anyhow::Error) -> (&'static str, Option<String>) {
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
                ureq::Error::Status(code, _) => ("gui-error-server", Some(code.to_string())),
                ureq::Error::Transport(t) => (transport_error(t.kind()), None),
            };
        }
        if let Some(t) = cause.downcast_ref::<ureq::Transport>() {
            return (transport_error(t.kind()), None);
        }
    }

    // 2) Typed I/O errors (file system and socket-level failures).
    for cause in err.chain() {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            use std::io::ErrorKind as K;
            let key = match io.kind() {
                K::PermissionDenied => "gui-error-permission",
                K::NotFound => "gui-error-not-found",
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
                | K::BrokenPipe => "gui-error-network",
                // Distinguish reads from writes by the surrounding context.
                _ if lower.contains("failed to read")
                    || lower.contains("failed to open")
                    || lower.contains("failed to seek") => "gui-error-disk-read",
                _ => "gui-error-disk-write",
            };
            return (key, None);
        }
    }

    // 3) Well-known message patterns.
    let key = if lower.contains("checksum mismatch")
        || lower.contains("sha-256 mismatch")
        || lower.contains("size mismatch")
        || lower.contains("invalid zip")
        || lower.contains("failed to decompress")
    {
        "gui-error-verification"
    } else if lower.contains("download stalled") || lower.contains("stalled after") {
        "gui-error-network"
    } else if (lower.contains("not a ") && lower.contains("client directory"))
        || lower.contains("no client data")
    {
        "gui-error-invalid-client"
    } else if lower.contains("invalid version number") {
        "gui-error-invalid-version"
    } else if lower.contains("no patches are published")
        || lower.contains("patch not found")
        || lower.contains("does not publish patch metadata")
    {
        "gui-error-no-patch"
    } else {
        // 4) Fallback.
        "gui-error-unknown"
    };
    (key, None)
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

    /// The "View log" link label is rendered directly by the window (not via
    /// `tr_error`), so guard it separately.
    #[test]
    fn view_log_key_present_in_all_catalogs() {
        for (name, src) in [("en", EN), ("zh-CN", ZH_CN), ("zh-TW", ZH_TW)] {
            assert!(
                parse(src).contains_key("gui-view-log"),
                "{name} is missing 'gui-view-log'"
            );
        }
    }

    /// The patcher's "pending Windows update" status line is likewise rendered
    /// directly by the window, so guard it separately too.
    #[test]
    fn pending_windows_update_key_present_in_all_catalogs() {
        for (name, src) in [("en", EN), ("zh-CN", ZH_CN), ("zh-TW", ZH_TW)] {
            assert!(
                parse(src).contains_key("gui-patcher-pending-windows-update"),
                "{name} is missing 'gui-patcher-pending-windows-update'"
            );
        }
    }

    /// The "Try to launch anyway" link and the status shown while that launch
    /// runs are rendered directly too.
    #[test]
    fn launch_anyway_keys_present_in_all_catalogs() {
        for (name, src) in [("en", EN), ("zh-CN", ZH_CN), ("zh-TW", ZH_TW)] {
            let cat = parse(src);
            for key in ["gui-patcher-launch-anyway", "gui-patcher-launching"] {
                assert!(cat.contains_key(key), "{name} is missing '{key}'");
            }
        }
    }

    /// The error ureq returns when nothing accepts the connection, wrapped in
    /// the same context layers as `cms::get_patch_data`. It connects to a
    /// loopback port that was just released, so no traffic leaves the host.
    fn connection_refused_error() -> anyhow::Error {
        use anyhow::Context;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("bind a loopback port")
            .port();
        ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_millis(500))
            .build()
            .get(&format!("http://127.0.0.1:{port}/"))
            .call()
            .context("HTTP request failed")
            .context("failed to fetch patch metadata")
            .expect_err("nothing should be listening on the released port")
    }

    /// An unreachable server is what offers the GUI's "Try to launch anyway"
    /// link, so it must be classified as `gui-error-network-connect`.
    #[test]
    fn refused_connection_is_a_network_connect_error() {
        let err = connection_refused_error();
        assert!(is_network_connect_error(&err), "misclassified: {err:#}");
        assert_eq!(tr_error(&err), tr("gui-error-network-connect", &[]));
    }

    /// Other failures keep their own messages and do not offer the link.
    #[test]
    fn other_errors_are_not_network_connect_errors() {
        use anyhow::Context;
        let response = ureq::Response::new(503, "Service Unavailable", "").unwrap();
        let server = anyhow::Error::new(ureq::Error::Status(503, response))
            .context("HTTP request failed");
        assert!(!is_network_connect_error(&server));
        assert_eq!(tr_error(&server), tr("gui-error-server", &["503"]));

        let denied: std::io::Result<()> =
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        let denied = denied.context("failed to write Base.wz").unwrap_err();
        assert!(!is_network_connect_error(&denied));
        assert_eq!(tr_error(&denied), tr("gui-error-permission", &[]));

        let unknown = anyhow::anyhow!("something unexpected");
        assert!(!is_network_connect_error(&unknown));
        assert_eq!(tr_error(&unknown), tr("gui-error-unknown", &[]));
    }
}
