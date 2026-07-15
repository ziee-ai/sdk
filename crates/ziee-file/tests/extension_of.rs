//! Crate-scoped unit test for `ziee_file::utils::extension_of` — the one public
//! blob-key helper the in-source tests (magic-sniff, zip-bomb, filesystem) don't
//! cover. It is the single source of truth for how `FileStorage::{save,load}`
//! name a blob, so save + load MUST agree on its result (case + dot-less +
//! empty). A drift here silently loses blobs on case-sensitive filesystems, so
//! the documented edge cases are locked in.

use ziee_file::utils::extension_of;

#[test]
fn takes_the_substring_after_the_last_dot_lowercased() {
    assert_eq!(extension_of("photo.PNG"), "png", "the extension is lowercased");
    assert_eq!(extension_of("archive.tar.gz"), "gz", "only the LAST segment");
    assert_eq!(extension_of("report.PDF"), "pdf");
}

#[test]
fn dotless_name_yields_the_whole_name_lowercased() {
    // Matches how `upload` keys a dot-less file (e.g. `Makefile`).
    assert_eq!(extension_of("Makefile"), "makefile");
}

#[test]
fn leading_dot_dotfile_yields_the_name_after_the_dot() {
    // `.bashrc` → `bashrc` (rsplit's last non-empty segment).
    assert_eq!(extension_of(".bashrc"), "bashrc");
}

#[test]
fn empty_or_trailing_dot_falls_back_to_bin() {
    assert_eq!(extension_of(""), "bin", "empty name → bin");
    assert_eq!(extension_of("trailingdot."), "bin", "a trailing dot → bin");
}
