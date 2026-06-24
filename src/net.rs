//! Shared HTTP agent construction.
//!
//! All network requests go through an [`ureq::Agent`] built here so the
//! optional insecure-TLS mode (`--allow-insecure`) can be applied uniformly.
//! When enabled, server certificates are not verified at all; this is unsafe
//! and only exists as an escape hatch for environments where the normal chain
//! cannot be validated (see [`agent_builder`]).

use std::sync::{Arc, OnceLock};

use ureq::AgentBuilder;

/// Return an [`AgentBuilder`], applying the insecure TLS config (no certificate
/// verification) when `allow_insecure` is set, and routing through `proxy` when
/// a proxy URL is given.
///
/// Callers can chain additional configuration (timeouts, etc.) before calling
/// `.build()`.
pub fn agent_builder(allow_insecure: bool, proxy: Option<&str>) -> AgentBuilder {
    let mut builder = AgentBuilder::new();
    if allow_insecure {
        builder = builder.tls_config(insecure_tls_config());
    }
    if let Some(url) = proxy {
        match ureq::Proxy::new(url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!(
                "warning: invalid proxy '{url}': {e}; connecting directly instead."
            ),
        }
    }
    builder
}

/// Build a ready-to-use agent for one-off metadata/control requests.
pub fn agent(allow_insecure: bool, proxy: Option<&str>) -> ureq::Agent {
    agent_builder(allow_insecure, proxy).build()
}

/// Resolve the `--proxy` argument into the proxy URL to use, or `None` for a
/// direct connection.
///
/// - `None` (flag absent): no proxy.
/// - `Some(None)` or `Some(Some(""))` (flag with no value): use the system proxy.
/// - `Some(Some(url))`: use `url`, defaulting to an `http://` scheme when none
///   is given.
///
/// The chosen proxy (or lack of one) is reported to stderr.
pub fn resolve_proxy(arg: Option<&Option<String>>) -> Option<String> {
    match arg {
        None => None,
        Some(value) => {
            let explicit = value.as_deref().map(str::trim).filter(|s| !s.is_empty());
            match explicit {
                Some(url) => {
                    let url = normalize_proxy_url(url);
                    eprintln!("using proxy: {url}");
                    Some(url)
                }
                None => match system_proxy() {
                    Some(url) => {
                        eprintln!("using system proxy: {url}");
                        Some(url)
                    }
                    None => {
                        eprintln!("no system proxy configured; connecting directly.");
                        None
                    }
                },
            }
        }
    }
}

/// Prepend a default `http://` scheme to a proxy address that lacks one.
fn normalize_proxy_url(url: &str) -> String {
    if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

/// Discover the system-configured proxy.
///
/// The common `*_PROXY` environment variables are checked first (in order of
/// specificity), then any OS-specific configuration.
fn system_proxy() -> Option<String> {
    for var in [
        "ALL_PROXY",
        "all_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
    ] {
        if let Ok(value) = std::env::var(var) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(normalize_proxy_url(value));
            }
        }
    }
    system_proxy_os()
}

/// Read the proxy from the Windows Internet Settings, if one is enabled.
#[cfg(windows)]
fn system_proxy_os() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;

    let settings = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
        .ok()?;

    let enabled: u32 = settings.get_value("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }

    let server: String = settings.get_value("ProxyServer").ok()?;
    parse_windows_proxy_server(&server)
}

/// No OS-specific proxy detection on non-Windows platforms (env vars only).
#[cfg(not(windows))]
fn system_proxy_os() -> Option<String> {
    None
}

/// Parse the Windows `ProxyServer` registry value into a single proxy URL.
///
/// The value is either a bare `host:port` (used for all protocols) or a
/// protocol-specific list like `http=host:port;https=host:port;socks=host:port`.
/// For the list form, the `https`, then `http`, then `socks` entry is preferred.
#[cfg(windows)]
fn parse_windows_proxy_server(server: &str) -> Option<String> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }

    if !server.contains('=') {
        return Some(normalize_proxy_url(server));
    }

    let mut http = None;
    let mut https = None;
    let mut socks = None;
    for part in server.split(';') {
        if let Some((proto, addr)) = part.split_once('=') {
            let addr = addr.trim();
            if addr.is_empty() {
                continue;
            }
            match proto.trim().to_ascii_lowercase().as_str() {
                "http" => http = Some(addr.to_owned()),
                "https" => https = Some(addr.to_owned()),
                "socks" => socks = Some(addr.to_owned()),
                _ => {}
            }
        }
    }

    if let Some(addr) = https.or(http) {
        return Some(normalize_proxy_url(&addr));
    }
    socks.map(|addr| {
        if addr.contains("://") {
            addr
        } else {
            format!("socks5://{addr}")
        }
    })
}

/// Lazily build (once) and return the shared insecure client config.
fn insecure_tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG.get_or_init(build_insecure_config).clone()
}

/// Construct a rustls client config that accepts any server certificate.
///
/// The signature-verification algorithms still come from the ring crypto
/// provider (the same one ureq uses), so only the certificate *trust* check is
/// disabled, not the TLS handshake itself.
fn build_insecure_config() -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .expect("ring provider supports the default protocol versions")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(NoCertVerification { provider }))
        .with_no_client_auth();
    Arc::new(config)
}

/// A certificate verifier that accepts every certificate without validation.
#[derive(Debug)]
struct NoCertVerification {
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for NoCertVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
