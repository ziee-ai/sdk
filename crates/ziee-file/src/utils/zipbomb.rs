//! Decompression-bomb pre-validation for ZIP-family uploads.
//!
//! OOXML containers (DOCX/XLSX/PPTX) and ODT/ODS are ZIP archives.
//! Without a pre-check, a small 16 MB upload can claim to expand to
//! tens of GB and exhaust memory when the office processor opens it.
//! Closes 05-file F-05 (High).
//!
//! Caller passes the raw upload bytes; we walk the central directory
//! and:
//!   - sum the declared uncompressed sizes
//!   - check the worst per-entry compression ratio
//! Returns Err if either limit is exceeded. We do NOT decompress
//! anything — the central-directory metadata is enough.

use std::io::Cursor;

/// Max total uncompressed size across all entries (256 MiB).
pub const MAX_UNCOMPRESSED_BYTES: u64 = 256 * 1024 * 1024;

/// Max ratio uncompressed/compressed per entry. Real DOCX/XLSX
/// typically compress at < 10:1; 200:1 leaves headroom for embedded
/// XML but flags the classic zip bomb.
pub const MAX_COMPRESSION_RATIO: u64 = 200;

#[derive(Debug)]
pub enum ZipBombError {
    OpenFailed(String),
    TotalSizeExceeded { declared: u64, cap: u64 },
    RatioExceeded { ratio: u64, cap: u64 },
}

impl std::fmt::Display for ZipBombError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OpenFailed(e) => write!(f, "Cannot open as ZIP archive: {}", e),
            Self::TotalSizeExceeded { declared, cap } => write!(
                f,
                "ZIP declares {} bytes uncompressed (cap is {})",
                declared, cap
            ),
            Self::RatioExceeded { ratio, cap } => write!(
                f,
                "ZIP entry compression ratio {}:1 exceeds cap of {}:1 \
                 (likely zip bomb)",
                ratio, cap
            ),
        }
    }
}

impl std::error::Error for ZipBombError {}

/// Validate a ZIP-family upload against the configured caps. Returns
/// Ok(()) if safe to hand to the office processor.
pub fn validate(bytes: &[u8]) -> Result<(), ZipBombError> {
    let cursor = Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| ZipBombError::OpenFailed(e.to_string()))?;

    let mut total: u64 = 0;
    for i in 0..archive.len() {
        let file = archive
            .by_index_raw(i)
            .map_err(|e| ZipBombError::OpenFailed(format!("entry {}: {}", i, e)))?;
        let uncompressed = file.size();
        let compressed = file.compressed_size().max(1); // /0 guard
        let ratio = uncompressed / compressed;
        if ratio > MAX_COMPRESSION_RATIO {
            return Err(ZipBombError::RatioExceeded {
                ratio,
                cap: MAX_COMPRESSION_RATIO,
            });
        }
        total = total.saturating_add(uncompressed);
        if total > MAX_UNCOMPRESSED_BYTES {
            return Err(ZipBombError::TotalSizeExceeded {
                declared: total,
                cap: MAX_UNCOMPRESSED_BYTES,
            });
        }
    }

    Ok(())
}

/// MIME prefixes that are ZIP-family containers. The processor pipeline
/// calls `validate` before extraction for any matching mime_type.
pub fn is_ooxml_or_odf(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.oasis.opendocument.text"
            | "application/vnd.oasis.opendocument.spreadsheet"
            | "application/zip"
    )
}
#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Cursor, Write};

    use zip::write::SimpleFileOptions;

    use zip::{CompressionMethod, ZipWriter};


    /// Build a single-entry zip whose `body` is Deflate-compressed.
    fn zip_with(body: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opts =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            w.start_file("entry.bin", opts).unwrap();
            w.write_all(body).unwrap();
            w.finish().unwrap();
        }
        buf
    }


    // audit id all-a745a3865cd6 — zip-bomb validation (the core decompression-
    // bomb guard) was untested. MIME smuggling is already covered by
    // file::utils::magic::tests (rejects_html_as_png / allows_html_as_html).
    #[test]
    fn validate_accepts_a_normal_low_ratio_zip() {
        // ~2 KiB of incompressible-ish text → ratio well under 200.
        let body: Vec<u8> = (0..2048u32).map(|i| (i % 251) as u8).collect();
        assert!(validate(&zip_with(&body)).is_ok(), "a normal zip must pass");
    }


    #[test]
    fn validate_rejects_a_high_ratio_bomb() {
        // 4 MiB of zeros compresses to a few KB → ratio >> MAX_COMPRESSION_RATIO.
        let bomb = zip_with(&vec![0u8; 4 * 1024 * 1024]);
        match validate(&bomb) {
            Err(ZipBombError::RatioExceeded { ratio, cap }) => {
                assert!(ratio > cap, "ratio {ratio} must exceed cap {cap}");
                assert_eq!(cap, MAX_COMPRESSION_RATIO);
            }
            other => panic!("a high-ratio zip must be rejected as RatioExceeded, got {other:?}"),
        }
    }


    #[test]
    fn validate_rejects_non_zip_bytes_as_open_failed() {
        match validate(b"this is plainly not a zip archive at all") {
            Err(ZipBombError::OpenFailed(_)) => {}
            other => panic!("non-zip bytes must fail to open, got {other:?}"),
        }
    }


    #[test]
    fn is_ooxml_or_odf_classifies_zip_family_mimes() {
        assert!(is_ooxml_or_odf("application/zip"));
        assert!(is_ooxml_or_odf(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        ));
        assert!(!is_ooxml_or_odf("text/plain"));
        assert!(!is_ooxml_or_odf("image/png"));
    }


    /// Build an in-memory ZIP with a single entry. We construct real
    /// archives (not hand-rolled bytes) so `validate` walks a genuine
    /// central directory exactly as it does for an uploaded OOXML file.
    fn make_zip(name: &str, method: CompressionMethod, data: &[u8]) -> Vec<u8> {
        let mut zw = ZipWriter::new(Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(method);
        zw.start_file(name, opts).unwrap();
        zw.write_all(data).unwrap();
        zw.finish().unwrap().into_inner()
    }


    #[test]
    fn rejects_high_ratio_entry_as_zip_bomb() {
        // 4 MiB of zeros deflates to a few hundred bytes → ratio well over
        // the 200:1 cap: the classic decompression bomb the guard exists for.
        let bytes = make_zip("bomb.bin", CompressionMethod::Deflated, &vec![0u8; 4 * 1024 * 1024]);
        match validate(&bytes) {
            Err(ZipBombError::RatioExceeded { ratio, cap }) => {
                assert_eq!(cap, MAX_COMPRESSION_RATIO);
                assert!(
                    ratio > MAX_COMPRESSION_RATIO,
                    "reported ratio {ratio} must exceed the cap {cap}"
                );
            }
            other => panic!("expected RatioExceeded, got {other:?}"),
        }
    }


    #[test]
    fn accepts_legitimate_low_ratio_archive() {
        // Stored (uncompressed) → ratio 1:1, total tiny: a real DOCX-shaped
        // archive that must pass the guard untouched.
        let bytes = make_zip(
            "document.xml",
            CompressionMethod::Stored,
            b"<?xml version=\"1.0\"?><document>hello</document>",
        );
        assert!(
            validate(&bytes).is_ok(),
            "a normal low-ratio archive must validate"
        );
    }


    #[test]
    fn non_zip_bytes_fail_to_open() {
        // A non-archive upload that slipped past the mime check must error
        // cleanly (OpenFailed), never panic.
        match validate(b"this is plainly not a zip archive at all") {
            Err(ZipBombError::OpenFailed(_)) => {}
            other => panic!("expected OpenFailed, got {other:?}"),
        }
    }


    #[test]
    fn is_ooxml_or_odf_matches_zip_family_only() {
        // The processor only runs `validate` for these container mimes.
        for m in [
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "application/vnd.oasis.opendocument.text",
            "application/vnd.oasis.opendocument.spreadsheet",
            "application/zip",
        ] {
            assert!(is_ooxml_or_odf(m), "{m} must be treated as a zip-family container");
        }
        for m in ["image/png", "text/plain", "application/pdf", ""] {
            assert!(!is_ooxml_or_odf(m), "{m} must NOT trigger zip-bomb validation");
        }
    }
}
