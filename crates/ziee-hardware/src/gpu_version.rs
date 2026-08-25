//! Pure, dependency-free parsers for GPU vendor version strings.
//!
//! These exist because host-capability detection used to be brittle
//! string-scraping of one vendor tool's human-readable banner, in ONE exact
//! byte sequence (`"CUDA Version:"`). NVIDIA driver 610 prints
//! `CUDA UMD Version: 13.3` instead, so that scrape found nothing, CUDA
//! detection returned `None`, and hosts with real GPUs silently selected the
//! **CPU** engine build. Worse, both `Driver Version` and `CUDA Version` are
//! now annotated by the driver itself as *"will be removed in CUDA 14.0"*, so
//! the same break is scheduled to recur.
//!
//! Everything here is a `fn(&str) -> Option<…>` with no I/O, so every real
//! `nvidia-smi` / `nvcc` output shape can be pinned by a unit test without a
//! GPU. The impure probes that feed them live in the callers
//! (`detection.rs` here, `llm_local_runtime::utils::gpu_detect` in ziee), and
//! BOTH call these — this module is the single implementation that replaced two
//! independently-drifting copies of the same broken scrape.
//!
//! **No dependencies on purpose.** The work is whitespace tokenisation over a
//! few short lines; pulling `regex` into this crate would add a direct
//! dependency to two separate workspaces and lockfiles to save nothing.

use std::fmt;

/// A `major[.minor]` version where the minor may be genuinely **unknown**.
///
/// `minor: None` is not the same fact as `minor: Some(0)`. A distro that ships
/// `libcudart.so.13` as the terminal real file tells us the major and nothing
/// else; recording that as `13.0` fabricates a number that would silently
/// become wrong the moment any caller started comparing minors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MajorMinor {
    /// Major version component.
    pub major: u32,
    /// Minor version component, or `None` when the source did not state one.
    pub minor: Option<u32>,
}

impl MajorMinor {
    /// Construct from parts.
    pub fn new(major: u32, minor: Option<u32>) -> Self {
        Self { major, minor }
    }

    /// Lower to the `(major, minor)` pair the older APIs take.
    ///
    /// Lossy by design, and deliberately confined to this one boundary: an
    /// unknown minor becomes `0`. Callers that consume the pair must not read
    /// the minor unless they have established the source actually stated one.
    pub fn as_pair(self) -> (u32, u32) {
        (self.major, self.minor.unwrap_or(0))
    }
}

impl fmt::Display for MajorMinor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.minor {
            Some(minor) => write!(f, "{}.{}", self.major, minor),
            None => write!(f, "{}.x", self.major),
        }
    }
}

/// Normalise one whitespace-delimited token for comparison: strip the
/// punctuation vendor tools attach to labels and table cells, then lowercase.
///
/// This is what lets ONE matcher handle `Version:` (colon attached, banner),
/// `version` + a separate `:` token (`nvidia-smi --version`), and `"CUDA`
/// (a key quoted inside prose). A token that is nothing but punctuation — `:`,
/// `|` — normalises to the empty string, which the matcher treats as a
/// skippable separator.
fn normalize_token(token: &str) -> String {
    token
        .trim_matches(|c: char| matches!(c, ':' | '"' | '\'' | ',' | '|' | '(' | ')'))
        .to_ascii_lowercase()
}

/// Parse a bare version token into [`MajorMinor`].
///
/// Deliberately strict about what counts as a version token, because the only
/// other thing standing between this and a fabricated version is the label
/// match in [`find_labeled_version`]. Accepted:
///
/// - a digit-led run of digits and dots — `13`, `13.3`, `13.3.29`
/// - the same with a single `V`/`v` prefix, which is nvcc's convention
///   (`V13.3.33`)
/// - either of those followed by punctuation — `13.3]`, `13.3,`
///
/// Rejected, and each of these is a real string that appears near a version in
/// vendor output: anything not digit-led after an optional `V` (`N/A`,
/// `Deprecated,`, `x86_64`, `H200`), and anything whose numeric run runs
/// straight into more alphanumerics or an underscore (`12GB`, `86_64`). The
/// trailing rule is what keeps a product name or a memory size from
/// impersonating a version if a future label ever matches next to one.
///
/// This is the guard that stops driver 610's
/// `CUDA version : Deprecated, see "CUDA UMD version" instead` from being read
/// as a version at all, and it is why `N/A` cannot reach a caller as a number.
pub fn parse_version_token(token: &str) -> Option<MajorMinor> {
    // At most one leading `V`/`v` (nvcc prints `V13.3.33`). Anything else
    // non-numeric in front means this is not a version token.
    let body = match token.strip_prefix(['V', 'v']) {
        Some(rest) => rest,
        None => token,
    };
    if !body.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    let end = body
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(body.len());
    let core = &body[..end];

    // The numeric run must END the token, modulo a closing punctuation mark.
    // An ALLOWLIST rather than a denylist of alphanumerics: a denylist let the
    // Bus-Id `00000000:03:00.0` through as major 0, because `:` is neither
    // alphanumeric nor `_`. Only characters that genuinely terminate a value
    // are accepted here.
    const VALUE_TERMINATORS: &[char] = &[']', ',', ')', ';', '%', '"', '\''];
    if let Some(next) = body[end..].chars().next()
        && !VALUE_TERMINATORS.contains(&next)
    {
        return None;
    }

    let mut parts = core.split('.').filter(|s| !s.is_empty());
    let major: u32 = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|s| s.parse::<u32>().ok());
    Some(MajorMinor { major, minor })
}

/// Find `key` in `text` as a case-insensitive **contiguous run** of
/// whitespace-delimited tokens, then parse the token that follows it as a
/// version. Matching is scoped to a single line.
///
/// Token-window matching, not a substring search and not
/// `line.split_once(':')`, because both of those get real output wrong:
///
/// - A substring search for `"CUDA Version:"` requires the colon to be
///   adjacent. `nvidia-smi --version` writes `CUDA UMD version    : 13.3`,
///   where it is not — and it lowercases the `v`.
/// - Splitting a line on its first `:` finds the wrong field. The legacy
///   driver-550 banner packs three pairs into one table cell
///   (`NVIDIA-SMI 550.90  Driver Version: 550.90  CUDA Version: 12.4`), so the
///   first colon belongs to `Driver Version`.
///
/// Requiring the key's tokens to appear **in sequence** is also what keeps the
/// driver version from being mistaken for the CUDA version: `NVIDIA-SMI
/// version : 610.43.02` cannot match a key beginning with a `cuda` token.
///
/// On a key match whose following token fails to parse, the scan **continues**
/// rather than giving up. This is load-bearing: on driver 610 the prose
/// `see "CUDA UMD version" instead` appears two lines ABOVE the real
/// `CUDA UMD version : 13.3`, so a first-match-wins scan would return `None`.
pub fn find_labeled_version(text: &str, key: &str) -> Option<MajorMinor> {
    find_labeled_version_inner(text, key, false)
}

/// As [`find_labeled_version`], but additionally requires the key to sit in a
/// real `label : value` position — a `:` must either end the key's last token
/// or appear between the key and the value.
///
/// This is what stops a version being read out of PROSE that happens to
/// contain the key. Driver 610 already prints
/// `… will be removed in CUDA 14.0. Use CUDA UMD Version instead]`; if a future
/// driver phrased that as `… removed in CUDA version 14.0.`, an unanchored
/// match would return **14.0** on a host capped at 13.3 and hand it a
/// `cuda14.x` artifact that cannot load. A wrong number is far worse than
/// `None`, so the CUDA keys pay for the anchor.
///
/// Not applied to `nvcc`'s `release 13.3,` or ROCm's labels, which are
/// legitimately colon-free.
fn find_labeled_version_colon_anchored(text: &str, key: &str) -> Option<MajorMinor> {
    find_labeled_version_inner(text, key, true)
}

fn find_labeled_version_inner(text: &str, key: &str, require_colon: bool) -> Option<MajorMinor> {
    let key_tokens: Vec<String> = key.split_whitespace().map(normalize_token).collect();
    if key_tokens.is_empty() {
        return None;
    }

    // Per LINE, not over the whole buffer. Tokenising the whole text would let
    // a key window straddle a newline and pick up the next line's number as
    // this line's value — not reachable on any output shape observed today,
    // but the "never fabricate a version" property should not rest on that.
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let normalized: Vec<String> = tokens.iter().map(|t| normalize_token(t)).collect();

        for start in 0..tokens.len() {
            let end = start + key_tokens.len();
            if end > tokens.len() || normalized[start..end] != key_tokens[..] {
                continue;
            }

            // `Version:` (colon glued to the label) satisfies the anchor; so
            // does a detached `:` token between label and value.
            let mut saw_colon = tokens[end - 1].ends_with(':');

            // Step over punctuation-only tokens (a detached `:`, a table `|`).
            let mut value_at = end;
            while value_at < tokens.len() && normalized[value_at].is_empty() {
                if tokens[value_at].contains(':') {
                    saw_colon = true;
                }
                value_at += 1;
            }

            if require_colon && !saw_colon {
                continue;
            }

            // Try the raw token first, then its normalised form. Without the
            // second attempt a value glued to a table border (`12.4|`) or
            // carrying a stray quote would be dropped, where the older
            // substring scraper accepted it — a needless regression.
            if let Some(version) = tokens.get(value_at).and_then(|t| {
                parse_version_token(t).or_else(|| parse_version_token(&normalize_token(t)))
            }) {
                return Some(version);
            }
            // Key matched but the value was prose ("Deprecated, see …") —
            // keep looking, here and on later lines.
        }
    }
    None
}

/// Labels carrying the driver's maximum supported CUDA **runtime**, strongest
/// first.
///
/// `CUDA UMD Version` is the modern spelling (driver R6xx and later).
/// `CUDA Version` is both the pre-R6xx spelling AND the deprecated-but-still-
/// populated field in `nvidia-smi -q` on R6xx, so it stays as a fallback. UMD
/// is tried first so a host printing both — which `nvidia-smi -q` does — wins
/// on the authoritative one rather than the one scheduled for removal.
pub const CUDA_VERSION_KEYS: &[&str] = &["cuda umd version", "cuda version"];

/// Parse the driver's maximum supported CUDA runtime out of **any**
/// `nvidia-smi` surface: the bare banner, `--version`, or `-q`.
///
/// One parser covers all three because [`find_labeled_version`] works on
/// whitespace tokens and is therefore indifferent to whether the output is an
/// ASCII table or a `key : value` list.
///
/// Note this is the *driver's* capability, not the installed toolkit's — it is
/// what decides whether a prebuilt CUDA engine binary can run at all.
pub fn parse_cuda_smi_version(stdout: &str) -> Option<MajorMinor> {
    CUDA_VERSION_KEYS
        .iter()
        .find_map(|key| find_labeled_version_colon_anchored(stdout, key))
}

/// Parse the toolkit version from `nvcc --version`
/// (`Cuda compilation tools, release 13.3, V13.3.33`).
///
/// This is the **toolkit**, i.e. what happens to be installed locally — an
/// upper bound on what can be built here, not a statement about what the
/// driver can execute. A toolkit newer than the driver over-reports, so
/// callers must treat this as weaker evidence than any `nvidia-smi` source.
pub fn parse_nvcc_version(stdout: &str) -> Option<MajorMinor> {
    find_labeled_version(stdout, "release")
}

/// Extract the version from a resolved `libcudart` file name.
///
/// `libcudart.so.13.3.29` → `13.3`; `libcudart.so.13` → major `13` with an
/// **unknown** minor; `libcudart.so` and other libraries → `None`.
///
/// Toolkit-derived, like [`parse_nvcc_version`]. Its value is that it needs no
/// subprocess at all, so it still answers on a host where `nvidia-smi` cannot
/// be spawned.
pub fn parse_cudart_soname(file_name: &str) -> Option<MajorMinor> {
    let rest = file_name.strip_prefix("libcudart.so.")?;
    parse_version_token(rest)
}

/// Extract a ROCm version from a versioned install directory name
/// (`rocm-6.1.2` or `/opt/rocm-6.1.2` → `6.1`).
///
/// The AMD mirror of the `libcudart` soname trick: AMD's packages install to a
/// version-stamped directory with `/opt/rocm` as a symlink to it, so
/// canonicalising that symlink recovers a version even when the `.info` files
/// a caller would normally read are absent.
///
/// UNVERIFIED against real hardware — no AMD GPU was available. Written
/// against the documented layout; callers use it parse-or-skip so a wrong
/// guess degrades to prior behaviour rather than producing a wrong answer.
pub fn parse_rocm_dir_name(name: &str) -> Option<MajorMinor> {
    let base = name.trim_end_matches('/').rsplit('/').next()?;
    let rest = base.strip_prefix("rocm-")?;
    parse_version_token(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Fixtures captured VERBATIM from a real host -------------------
    // 4x NVIDIA H200 NVL, driver/KMD 610.43.02, CUDA UMD 13.3, toolkit 13.3.33.
    // These are the exact bytes the tools emitted, not hand-written
    // approximations — that is the whole point of keeping them here.

    /// `nvidia-smi --version`. Note the LOWERCASE `version` in every key, the
    /// DETACHED colon, and the two `Deprecated, see "…" instead` values that
    /// sit ABOVE the real one.
    const SMI_VERSION_610: &str = concat!(
        "NVIDIA-SMI version  : 610.43.02\n",
        "NVML version        : 610.43\n",
        "DRIVER version      : Deprecated, see \"KMD version\" instead\n",
        "CUDA version        : Deprecated, see \"CUDA UMD version\" instead\n",
        "KMD version         : 610.43.02\n",
        "CUDA UMD version    : 13.3\n",
    );

    /// `nvidia-smi -q` header. Capitalised `Version`, detached colon, and a
    /// bracketed deprecation suffix glued to the value.
    const SMI_QUERY_610: &str = concat!(
        "Driver Version                   : 610.43.02 [Deprecated; will be removed in CUDA 14.0. Use KMD Version instead]\n",
        "CUDA Version                     : 13.3 [Deprecated; will be removed in CUDA 14.0. Use CUDA UMD Version instead]\n",
        "KMD Version                      : 610.43.02\n",
        "CUDA UMD Version                 : 13.3\n",
    );

    /// The driver-610 banner — the exact input that produced the bug report.
    const SMI_BANNER_610: &str =
        "| NVIDIA-SMI 610.43.02              KMD Version: 610.43.02     CUDA UMD Version: 13.3     |";

    /// The legacy driver-550 banner. Must keep working.
    const SMI_BANNER_550: &str =
        "| NVIDIA-SMI 550.90  Driver Version: 550.90  CUDA Version: 12.4 |";

    const NVCC_133: &str = concat!(
        "nvcc: NVIDIA (R) Cuda compiler driver\n",
        "Copyright (c) 2005-2026 NVIDIA Corporation\n",
        "Built on Fri_Apr_24_07:22:02_PM_PDT_2026\n",
        "Cuda compilation tools, release 13.3, V13.3.33\n",
        "Build cuda_13.3.r13.3/compiler.37862127_0\n",
    );

    fn mm(major: u32, minor: u32) -> MajorMinor {
        MajorMinor::new(major, Some(minor))
    }

    // ---- The reported bug ---------------------------------------------

    #[test]
    fn cuda_version_from_610_banner() {
        // THE bug: the literal "CUDA Version:" occurs zero times here.
        assert_eq!(parse_cuda_smi_version(SMI_BANNER_610), Some(mm(13, 3)));
    }

    #[test]
    fn cuda_version_from_smi_version_flag() {
        assert_eq!(parse_cuda_smi_version(SMI_VERSION_610), Some(mm(13, 3)));
    }

    #[test]
    fn cuda_version_from_smi_query() {
        assert_eq!(parse_cuda_smi_version(SMI_QUERY_610), Some(mm(13, 3)));
    }

    #[test]
    fn prefers_umd_over_legacy_cuda_field() {
        // `nvidia-smi -q` on R6xx prints BOTH. The legacy field is the one
        // NVIDIA says is going away, so the UMD field must win. A parser that
        // merely "found a CUDA version" would return the wrong number here
        // rather than nothing, which is why this is asserted separately.
        let both = "CUDA Version: 12.4\nCUDA UMD Version: 13.3\n";
        assert_eq!(parse_cuda_smi_version(both), Some(mm(13, 3)));
    }

    // ---- Regression guards: hosts that work today must keep working ----

    #[test]
    fn cuda_version_from_550_banner_still_works() {
        assert_eq!(parse_cuda_smi_version(SMI_BANNER_550), Some(mm(12, 4)));
    }

    #[test]
    fn parse_cuda_smi_version_rejects_prose() {
        assert_eq!(parse_cuda_smi_version("no cuda here"), None);
        assert_eq!(parse_cuda_smi_version(""), None);
    }

    // ---- Never fabricate a version ------------------------------------

    #[test]
    fn driver_version_is_never_read_as_cuda_version() {
        // The naive fix — "search for any `version` label" — returns 610.43
        // here. Requiring the `cuda` token to precede the label is the guard.
        let driver_only = "NVIDIA-SMI version  : 610.43.02\nNVML version        : 610.43\n";
        assert_eq!(parse_cuda_smi_version(driver_only), None);
    }

    #[test]
    fn deprecated_placeholder_yields_no_version() {
        let deprecated = "CUDA version : Deprecated, see \"CUDA UMD version\" instead\n";
        assert_eq!(parse_cuda_smi_version(deprecated), None);
    }

    #[test]
    fn quoted_key_in_prose_does_not_abort_the_scan() {
        // Exactly the driver-610 ordering: an unparseable match for the key,
        // then the real value later. First-match-wins would return None.
        let text = concat!(
            "CUDA version        : Deprecated, see \"CUDA UMD version\" instead\n",
            "CUDA UMD version    : 13.3\n",
        );
        assert_eq!(parse_cuda_smi_version(text), Some(mm(13, 3)));
    }

    #[test]
    fn na_is_not_a_version() {
        assert_eq!(parse_cuda_smi_version("CUDA Version: N/A"), None);
    }

    // ---- Token parsing -------------------------------------------------

    #[test]
    fn parse_version_token_table() {
        assert_eq!(parse_version_token("13.3"), Some(mm(13, 3)));
        assert_eq!(parse_version_token("13"), Some(MajorMinor::new(13, None)));
        assert_eq!(parse_version_token("13.3.29"), Some(mm(13, 3)));
        assert_eq!(parse_version_token("V13.3.33"), Some(mm(13, 3)));
        assert_eq!(parse_version_token("13.3]"), Some(mm(13, 3)));
        assert_eq!(parse_version_token("13.3,"), Some(mm(13, 3)));

        for junk in ["", "Deprecated,", "N/A", "Not", "unknown", "-"] {
            assert_eq!(parse_version_token(junk), None, "{junk:?} is not a version");
        }
    }

    #[test]
    fn parse_version_token_rejects_lookalikes_from_real_smi_output() {
        // Every one of these appears in `nvidia-smi -q` output near a label.
        // No CURRENT key lands next to them, but the "never fabricate a
        // version" property must not depend on that staying true.
        for lookalike in [
            "x86_64",  // Product Architecture / CPU arch
            "H200",    // Product Name : NVIDIA H200 NVL
            "12GB",    // memory sizes
            "86_64",   // the tail of a split arch token
            "P0",      // performance state
            "00000000:03:00.0", // Bus-Id
        ] {
            assert_eq!(
                parse_version_token(lookalike),
                None,
                "{lookalike:?} must not parse as a version"
            );
        }
    }

    #[test]
    fn product_name_line_is_not_read_as_a_cuda_version() {
        // The H200's own name contains a number. Guards the combination of a
        // permissive token parse with a future label change.
        let q = "Product Name                     : NVIDIA H200 NVL\n";
        assert_eq!(parse_cuda_smi_version(q), None);
    }

    #[test]
    fn prose_containing_the_key_is_not_read_as_a_version() {
        // Hypothetical future phrasing of the deprecation notice driver 610
        // already prints. Unanchored, this returns 14.0 on a 13.3 host and
        // would install a cuda14 build that cannot load.
        let prose = "CUDA Version : 13.3 [Deprecated; will be removed in CUDA version 14.0.]\n";
        assert_eq!(parse_cuda_smi_version(prose), Some(mm(13, 3)));

        // With only the prose and no real field, there must be NO version.
        let prose_only = "Note: support will be removed in CUDA version 14.0.\n";
        assert_eq!(parse_cuda_smi_version(prose_only), None);
    }

    #[test]
    fn value_glued_to_a_table_border_still_parses() {
        // The raw token is `12.4|`; `|` is not a value terminator, so this
        // only works because the normalised token is tried as a fallback.
        assert_eq!(
            parse_cuda_smi_version("| NVIDIA-SMI 550.90  CUDA Version: 12.4|"),
            Some(mm(12, 4))
        );
    }

    #[test]
    fn a_key_window_does_not_straddle_a_newline() {
        // "cuda umd version" must not be assembled from tokens on two lines,
        // and a value on the NEXT line must not be adopted as this line's.
        let split = "CUDA UMD\nversion : 13.3\n";
        assert_eq!(parse_cuda_smi_version(split), None);

        let dangling = "CUDA UMD Version:\n13.3\n";
        assert_eq!(parse_cuda_smi_version(dangling), None);
    }

    // ---- Toolkit-derived sources ---------------------------------------

    #[test]
    fn parse_nvcc_version_release() {
        assert_eq!(parse_nvcc_version(NVCC_133), Some(mm(13, 3)));
    }

    #[test]
    fn parse_nvcc_version_rejects_banner_only() {
        let banner = "nvcc: NVIDIA (R) Cuda compiler driver\nCopyright (c) 2005-2026 NVIDIA Corporation\n";
        assert_eq!(parse_nvcc_version(banner), None);
    }

    #[test]
    fn parse_cudart_soname_full() {
        assert_eq!(parse_cudart_soname("libcudart.so.13.3.29"), Some(mm(13, 3)));
    }

    #[test]
    fn parse_cudart_soname_major_only() {
        // The minor is genuinely unknown here and must NOT be invented as 0.
        assert_eq!(
            parse_cudart_soname("libcudart.so.13"),
            Some(MajorMinor::new(13, None))
        );
    }

    #[test]
    fn parse_cudart_soname_rejects_foreign() {
        for junk in ["libcudart.so", "libcublas.so.13.3", "libcudart.so.x", ""] {
            assert_eq!(parse_cudart_soname(junk), None, "{junk:?}");
        }
    }

    // ---- ROCm (UNVERIFIED against hardware) ----------------------------

    #[test]
    fn parse_rocm_dir_name_table() {
        assert_eq!(parse_rocm_dir_name("rocm-6.1.2"), Some(mm(6, 1)));
        assert_eq!(parse_rocm_dir_name("/opt/rocm-6.1.2"), Some(mm(6, 1)));
        assert_eq!(parse_rocm_dir_name("/opt/rocm-6.1.2/"), Some(mm(6, 1)));
        assert_eq!(parse_rocm_dir_name("rocm"), None);
        assert_eq!(parse_rocm_dir_name("rocm-"), None);
        assert_eq!(parse_rocm_dir_name("/opt/rocm"), None);
    }

    #[test]
    fn rocm_smi_version_line() {
        // UNVERIFIED string shape — no AMD hardware was available.
        let out = "ROCM-SMI-LIB version: 6.1.2\n";
        assert_eq!(find_labeled_version(out, "rocm-smi-lib version"), Some(mm(6, 1)));
    }

    // ---- Display / lowering --------------------------------------------

    #[test]
    fn display_marks_an_unknown_minor() {
        assert_eq!(mm(13, 3).to_string(), "13.3");
        assert_eq!(MajorMinor::new(13, None).to_string(), "13.x");
    }

    #[test]
    fn as_pair_fills_unknown_minor_with_zero() {
        assert_eq!(mm(13, 3).as_pair(), (13, 3));
        assert_eq!(MajorMinor::new(13, None).as_pair(), (13, 0));
    }
}
