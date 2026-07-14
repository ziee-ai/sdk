//! Magic-byte MIME sniffing.
//!
//! Closes 05-file F-04 (High): the upload handler trusts the
//! extension-derived MIME (`mime_guess::from_ext`), which lets an
//! attacker upload `evil.png` whose content is HTML. The browser then
//! sniffs the HTML and renders the file in the user's origin —
//! stored-XSS.
//!
//! This module sniffs the file's first ~16 bytes and returns the
//! canonical MIME for the well-known formats we care about. Callers
//! compare it against the extension-derived MIME and:
//!   - prefer the sniffed value when known,
//!   - reject the upload outright when the sniffed value is HTML/JS
//!     but the extension claims an image / video / pdf / model file
//!     (a clear smuggling attempt).
//!
//! Coverage is deliberately narrow — adding a full `infer` crate
//! dep would broaden the surface for little gain at this stage.

/// Sniff a MIME type from the first bytes of a file. Returns `None`
/// when the signature isn't recognised — callers fall back to the
/// extension-derived MIME.
pub fn sniff_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }

    // Image formats
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some("image/png");
    }
    if bytes.starts_with(b"\xff\xd8\xff") {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.starts_with(b"RIFF") && bytes.len() >= 12 && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(b"BM") {
        return Some("image/bmp");
    }

    // Document formats
    if bytes.starts_with(b"%PDF-") {
        return Some("application/pdf");
    }
    // ZIP-family (DOCX/XLSX/PPTX/ODT/etc — caller still validates via
    // the office processor; this just rejects HTML-disguised-as-zip).
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some("application/zip");
    }

    // Archive / compression
    if bytes.starts_with(b"\x1f\x8b") {
        return Some("application/gzip");
    }

    // HTML — leading whitespace + `<` then alpha. Conservative match
    // (lots of true-positive variants but very rare in non-HTML payloads).
    let trimmed = trim_leading_ws(bytes);
    if trimmed.starts_with(b"<!DOCTYPE")
        || trimmed.starts_with(b"<!doctype")
        || trimmed.starts_with(b"<html")
        || trimmed.starts_with(b"<HTML")
        || trimmed.starts_with(b"<script")
        || trimmed.starts_with(b"<SCRIPT")
    {
        return Some("text/html");
    }

    None
}

fn trim_leading_ws(bytes: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
        i += 1;
    }
    &bytes[i..]
}

/// Check whether a sniffed MIME contradicts the extension-derived MIME
/// in a way that suggests smuggling (HTML/JS disguised as a media or
/// document type). Returns `Some(reason)` to reject the upload, `None`
/// to allow.
pub fn smuggling_rejection(sniffed: Option<&str>, claimed_mime: &str) -> Option<&'static str> {
    let Some(sniffed) = sniffed else {
        return None;
    };
    // Block HTML disguised as anything-but-html / anything-but-text.
    if sniffed == "text/html"
        && !claimed_mime.starts_with("text/html")
        && !claimed_mime.starts_with("text/plain")
    {
        return Some(
            "File content is HTML but extension claims a non-HTML type \
             (likely XSS smuggling attempt)",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniffs_png() {
        assert_eq!(sniff_mime(b"\x89PNG\r\n\x1a\n......"), Some("image/png"));
    }

    #[test]
    fn sniffs_html_with_leading_ws() {
        assert_eq!(
            sniff_mime(b"  \n  <html><body>hi"),
            Some("text/html")
        );
    }

    #[test]
    fn rejects_html_as_png() {
        assert_eq!(
            smuggling_rejection(Some("text/html"), "image/png"),
            Some("File content is HTML but extension claims a non-HTML type \
                  (likely XSS smuggling attempt)")
        );
    }

    #[test]
    fn allows_html_as_html() {
        assert_eq!(smuggling_rejection(Some("text/html"), "text/html"), None);
    }

    #[test]
    fn unknown_signature_is_none() {
        assert_eq!(sniff_mime(b"\x00\x01\x02\x03random"), None);
    }

    // Scientific/genomics data files (`.rds`, etc.) must upload. A gzip-framed
    // `.rds` sniffs as gzip; an uncompressed one has no known signature. Neither
    // is HTML, so `smuggling_rejection` must allow both regardless of the
    // extension-claimed MIME (`mime_guess` yields application/octet-stream for
    // `.rds`). Locks in the behavior so a future sniff tightening can't silently
    // reject data files.
    #[test]
    fn gzip_framed_rds_sniffs_gzip_and_is_allowed() {
        // gzip magic 0x1f 0x8b, deflate method, then arbitrary bytes.
        let rds = b"\x1f\x8b\x08\x00\x00\x00\x00\x00rest-of-rds";
        assert_eq!(sniff_mime(rds), Some("application/gzip"));
        assert_eq!(
            smuggling_rejection(Some("application/gzip"), "application/octet-stream"),
            None
        );
    }

    #[test]
    fn uncompressed_binary_rds_is_unknown_and_allowed() {
        // Uncompressed R serialization starts with "X\n" / "RDX"; no signature
        // we recognise → None → allowed (not an HTML smuggling attempt).
        let rds = b"X\n\x00\x00\x00\x03random-binary-payload";
        assert_eq!(sniff_mime(rds), None);
        assert_eq!(
            smuggling_rejection(None, "application/octet-stream"),
            None
        );
    }
}
