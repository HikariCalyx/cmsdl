//! CMS (Mainland China region) specific control-file logic.
//!
//! The launcher control file (`v3ctrl.xml`) stores several "md5key" values
//! that are RSA-encrypted with the matching private key. They can be recovered
//! by applying the public RSA operation (`m = c^e mod n`) and stripping the
//! PKCS#1 v1.5 padding. The challenge key used by the launcher is built from
//! the first half of the decrypted `client` key and the second half of the
//! decrypted `server-let` key.

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine;
use chrono::{FixedOffset, Utc};
use md5::{Digest, Md5};
use rsa::pkcs8::DecodePublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{BigUint, RsaPublicKey};

/// URL of the CMS launcher control file.
const CTRL_XML_URL: &str = "https://downloader.dorado.sdo.com/v3launcher/5/v3ctrl.xml";

/// Host serving the signed client download files.
const DOWNLOAD_HOST: &str = "https://mxdver0.jijiagames.com";

/// Path of the client file list, relative to the download host.
const CLIENT_FILE_LIST_PATH: &str = "/v3client/build/5/8848/apppc/1020/client_all_files_list.dat";

/// Base64-encoded DER (SubjectPublicKeyInfo) of the RSA public key used to
/// decrypt the control-file `md5key` values.
const RSA_PUBLIC_KEY_DER_B64: &str = "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQCbHyTRH+DWw75sjRijIHobLf2rMNE3ob36WrpZePKU8V9ePQlLXvCVCQq4uFSF2KDtJwm9IBoSHzka36c38yMfYk/+FO/uIjcWOhgyzGbDajHQqtsKSTGqCWuoDdJiBDdb/fAVyvUToTaRFwpc8hYLn62iO8zhpevAa4tWgHDPFwIDAQAB";

/// Parse the embedded RSA public key.
fn public_key() -> Result<RsaPublicKey> {
    let der = base64::engine::general_purpose::STANDARD
        .decode(RSA_PUBLIC_KEY_DER_B64)
        .context("failed to base64-decode RSA public key")?;
    RsaPublicKey::from_public_key_der(&der).context("failed to parse RSA public key")
}

/// Fetch the CMS control file and compute the launcher challenge key.
///
/// The challenge key is the concatenation of the first 16 characters of the
/// decrypted `client` md5key and the last 16 characters of the decrypted
/// `server-let` md5key.
pub fn get_challenge_key() -> Result<String> {
    let xml = fetch_ctrl_xml().context("failed to fetch v3ctrl.xml")?;
    let (server_let_hex, client_hex) =
        extract_md5keys(&xml).context("failed to parse md5keys from v3ctrl.xml")?;

    let public_key = public_key()?;

    let server_let = decrypt_md5key(&public_key, &server_let_hex)
        .context("failed to decrypt server-let md5key")?;
    let client =
        decrypt_md5key(&public_key, &client_hex).context("failed to decrypt client md5key")?;

    build_challenge_key(&client, &server_let)
}

/// Download the control file as text.
fn fetch_ctrl_xml() -> Result<String> {
    // The document declares `encoding="gbk"`, but every value we care about is
    // ASCII hex, so a lossy UTF-8 decode is sufficient.
    http_get_text(CTRL_XML_URL).context("HTTP request failed")
}

/// Perform a GET request and return the response body as (lossy) UTF-8 text.
fn http_get_text(url: &str) -> Result<String> {
    let mut reader = ureq::get(url).call().context("HTTP request failed")?.into_reader();

    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf).context("failed to read response body")?;

    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// Extract the `server-let` and `client` md5key hex strings from the XML.
fn extract_md5keys(xml: &str) -> Result<(String, String)> {
    // roxmltree only supports UTF-8; rewrite the declared encoding so the
    // (ASCII) document parses cleanly.
    let sanitized = xml.replacen("encoding=\"gbk\"", "encoding=\"utf-8\"", 1);
    let doc = roxmltree::Document::parse(&sanitized).context("invalid XML")?;

    let server_let = find_md5key(&doc, "server-let")
        .ok_or_else(|| anyhow!("missing <server-let><md5key> element"))?;
    let client =
        find_md5key(&doc, "client").ok_or_else(|| anyhow!("missing <client><md5key> element"))?;

    Ok((server_let, client))
}

/// Find the text of `<parent><md5key>...</md5key></parent>` for the given parent tag.
fn find_md5key(doc: &roxmltree::Document, parent_tag: &str) -> Option<String> {
    doc.descendants()
        .find(|n| n.has_tag_name(parent_tag))?
        .children()
        .find(|c| c.has_tag_name("md5key"))?
        .text()
        .map(|t| t.trim().to_owned())
}

/// Apply the public RSA operation to a hex-encoded ciphertext and strip the
/// PKCS#1 v1.5 padding, returning the decrypted UTF-8 payload.
fn decrypt_md5key(public_key: &RsaPublicKey, hex_ciphertext: &str) -> Result<String> {
    let cipher_bytes = hex::decode(hex_ciphertext).context("md5key is not valid hex")?;

    // m = c^e mod n
    let c = BigUint::from_bytes_be(&cipher_bytes);
    let m = c.modpow(public_key.e(), public_key.n());

    // Left-pad to the modulus size so the PKCS#1 block structure is intact.
    let mut block = m.to_bytes_be();
    let key_size = public_key.size();
    if block.len() < key_size {
        let mut padded = vec![0u8; key_size - block.len()];
        padded.extend_from_slice(&block);
        block = padded;
    }

    let payload = pkcs1_unpad(&block)?;
    String::from_utf8(payload).context("decrypted md5key is not valid UTF-8")
}

/// Strip PKCS#1 v1.5 padding (block type 01 or 02) and return the payload.
///
/// The block layout is: `00 || BT || PS || 00 || payload`, where `BT` is the
/// block type and `PS` is the padding string.
fn pkcs1_unpad(block: &[u8]) -> Result<Vec<u8>> {
    if block.len() < 11 || block[0] != 0x00 || (block[1] != 0x01 && block[1] != 0x02) {
        bail!("invalid PKCS#1 padding");
    }

    // Find the 0x00 separator that terminates the padding string.
    let sep = block[2..]
        .iter()
        .position(|&b| b == 0x00)
        .map(|p| p + 2)
        .ok_or_else(|| anyhow!("PKCS#1 padding separator not found"))?;

    Ok(block[sep + 1..].to_vec())
}

/// Build the challenge key from the decrypted `client` and `server-let` keys.
fn build_challenge_key(client: &str, server_let: &str) -> Result<String> {
    if client.len() < 16 || server_let.len() < 16 {
        bail!("decrypted keys are too short to build a challenge key");
    }

    let client_head = &client[..16];
    let server_let_tail = &server_let[server_let.len() - 16..];

    Ok(format!("{client_head}{server_let_tail}"))
}

/// Return the current UTC+8 time as a number in `yyyyMMddHHmm` format.
///
/// For example, `2026-06-23 18:38` (UTC+8) is returned as `202606231838`.
pub fn get_current_utc8_time() -> u64 {
    // UTC+8 is a fixed offset of 8 hours east, so it is unaffected by DST.
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8 is a valid offset");
    let now = Utc::now().with_timezone(&offset);
    now.format("%Y%m%d%H%M")
        .to_string()
        .parse()
        .expect("yyyyMMddHHmm is always a valid number")
}

/// Build the signed URL for the client file list and download its contents.
///
/// The path is signed with an MD5 of `<challengeCode><utc8Time><path>`, and the
/// resulting URL is `<host>/<utc8Time>/<md5>/<path>`.
pub fn get_client_file_list() -> Result<String> {
    let utc8_time = get_current_utc8_time();
    let challenge_code = get_challenge_key().context("failed to obtain challenge code")?;

    let url = build_client_file_list_url(&challenge_code, utc8_time);
    http_get_text(&url).context("failed to download client file list")
}

/// Build the signed client-file-list URL from the challenge code and time value.
fn build_client_file_list_url(challenge_code: &str, utc8_time: u64) -> String {
    let signature_input = format!("{challenge_code}{utc8_time}{CLIENT_FILE_LIST_PATH}");
    let signature = md5_hex(&signature_input);

    format!("{DOWNLOAD_HOST}/{utc8_time}/{signature}{CLIENT_FILE_LIST_PATH}")
}

/// Compute the lowercase hex MD5 digest of `input`.
fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hex values taken from a real v3ctrl.xml response.
    const SERVER_LET_HEX: &str = "451a58c3de16c4d133d4d3fa8fee0e4e0de76b77a4156224ca5ea186f22db1f4af56427d5f0ee7bf6a7f96401d1890a158f26d7542d170815b5e81514a869bef8bfb131109281b0125d7904597671aa62637c5fe9bcee704d8893ac5f0f2358eb82749b08ab493d526af2fc30e0aa8d7bd1677e945483db7570957910bd5ea48";
    const CLIENT_HEX: &str = "95338a729569daf8fdce6e0734be13296679204adf31007897a615b3b43c8597ac61f6979a97254cde9e8f45355221814cc69d1d1ab6d754a16982f078baf43d74ade36c8c494992318a97a62954587e2c12fb6f1d2e6553fbb1e46b3b53af6de95b8dda496f50b85652f7cde6612af53e770959b13254c4cf1031e45d590f10";

    fn key() -> RsaPublicKey {
        public_key().unwrap()
    }

    #[test]
    fn decrypts_server_let_md5key() {
        let decrypted = decrypt_md5key(&key(), SERVER_LET_HEX).unwrap();
        assert_eq!(decrypted, "89T532jrQxUen6375E983L7758vajQSz");
    }

    #[test]
    fn decrypts_client_md5key() {
        let decrypted = decrypt_md5key(&key(), CLIENT_HEX).unwrap();
        assert_eq!(decrypted, "A9D8rTV72Fh7O8w7XPLp672657844VeS");
    }

    #[test]
    fn builds_expected_challenge_key() {
        let server_let = decrypt_md5key(&key(), SERVER_LET_HEX).unwrap();
        let client = decrypt_md5key(&key(), CLIENT_HEX).unwrap();
        let challenge = build_challenge_key(&client, &server_let).unwrap();
        assert_eq!(challenge, "A9D8rTV72Fh7O8w75E983L7758vajQSz");
    }
}
