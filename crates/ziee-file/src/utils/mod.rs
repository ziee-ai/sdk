// File-store utilities (blob-key derivation + upload-security validators).
//
// The processing/format helpers (pandoc/pdfium/spreadsheet/export/embedded) are
// NOT part of the store — they stay app-side. Only the domain-agnostic
// blob-key + magic-sniff + zip-bomb validators live here.

/// Derive the on-disk extension that `FileStorage::{save,load}_original` use to
/// name a blob (`originals/{user}/{id}.{ext}`): the substring after the LAST
/// `.`, **lowercased** to match the upload + download handlers (which lowercase
/// the same way) — save and load MUST agree on case, which matters on
/// case-sensitive filesystems. A dot-less name yields the whole name (e.g.
/// `Makefile` → `makefile`, `.bashrc` → `bashrc`), exactly as `upload` keys it;
/// `"bin"` only for an empty / trailing-dot name. Single source of truth — used
/// by the sandbox / mcp / provider-routing read paths so a file's blob is always
/// found regardless of which path wrote it.
pub fn extension_of(filename: &str) -> String {
    filename
        .rsplit('.')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("bin")
        .to_lowercase()
}

pub mod magic;
pub mod zipbomb;
