//! Per-worktree build/test database isolation.
//!
//! Multiple git worktrees of a repo share ONE pgvector server (the
//! docker-compose build container on 127.0.0.1:54321). `build.rs` wipes +
//! re-migrates the `public` schema of the database it connects to on EVERY
//! build, and the integration harness creates a `ziee_test_template*` database
//! on the same cluster. With a single shared database name, two worktrees
//! building/testing concurrently clobber each other (schema wiped mid-build,
//! `template does not exist`, duplicate-key, etc.).
//!
//! This module derives a STABLE, AUTOMATIC per-worktree suffix from the
//! worktree root path so each worktree gets its own build database and its own
//! template database — no manual `DATABASE_URL` per worktree, no CI wiring.
//!
//! ## The sentinel
//!
//! `src-app/.cargo/config.toml` unconditionally sets `DATABASE_URL` to the
//! committed docker-compose default (so sqlx macros work out of the box). That
//! means `DATABASE_URL` is never literally unset inside a cargo invocation. We
//! therefore treat that EXACT committed string as the "please auto-isolate"
//! sentinel:
//!
//!   * `DATABASE_URL == DEFAULT_BUILD_DB_URL` (or genuinely unset)  → derive a
//!     per-worktree database on the same cluster.
//!   * any OTHER `DATABASE_URL`  → a deliberate operator/CI override; honored
//!     byte-for-byte, behavior unchanged.
//!
//! Opt out of auto-isolation entirely with `ZIEE_BUILD_DB_PERWORKTREE=0`
//! (falls back to the shared default — the historical behavior).
//!
//! Chunk sdk-batteries (decision #11) moved this module out of ziee's
//! `server/build_helper/` into the SDK so both the ziee app AND any second app
//! reach the identical suffix derivation via `ziee_framework::build_support`.

#![allow(dead_code)]

/// The committed docker-compose default — the sentinel that means
/// "no explicit override, safe to auto-isolate per worktree".
pub const DEFAULT_BUILD_DB_URL: &str =
    "postgresql://postgres:password@127.0.0.1:54321/postgres";

/// host:port of the committed local docker-compose build cluster. Any
/// DATABASE_URL pointing here is the shared local build DB — safe (and
/// desirable) to auto-isolate per worktree, regardless of which database
/// name it carries (the build itself rewrites the db to `ziee_build_<key>`
/// via `cargo:rustc-env`, so the runtime value is no longer the literal
/// sentinel).
pub const LOCAL_BUILD_CLUSTER: &str = "127.0.0.1:54321";

/// FNV-1a 64-bit hash → first 8 hex chars. Stable across processes,
/// platforms, and rebuilds (no `Hash`/`DefaultHasher` randomization).
fn stable_suffix(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in input.bytes() {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (hash ^ (hash >> 32)) as u32)
}

/// Strip everything from `/src-app` onward so the server crate
/// (`.../src-app/server`) and the desktop crate
/// (`.../src-app/desktop/tauri`) of the SAME worktree map to the SAME
/// root — one key per worktree, not per crate. Handles both path
/// separators for Windows.
fn worktree_root(manifest_dir: &str) -> String {
    for marker in ["/src-app", "\\src-app"] {
        if let Some(i) = manifest_dir.find(marker) {
            return manifest_dir[..i].to_string();
        }
    }
    manifest_dir.to_string()
}

/// Deterministic per-worktree key derived from the worktree root path.
/// `manifest_dir` is `CARGO_MANIFEST_DIR` (server or desktop crate).
pub fn worktree_key(manifest_dir: &str) -> String {
    stable_suffix(&worktree_root(manifest_dir))
}

/// The per-worktree key derived from the CURRENT WORKING DIRECTORY's worktree
/// root, for callers that are NOT a build.rs (no `CARGO_MANIFEST_DIR`) — e.g. a
/// dev/test harness or a script. Resolves the worktree root the same way
/// `worktree_key` does (strip from `/src-app` onward), so a script run anywhere
/// under a worktree derives the SAME key as that worktree's build. Falls back to
/// the raw cwd string on error (still stable per invocation location).
///
/// This is the Rust half of the unified run-key; the byte-identical Node twin is
/// `sdk/packages/gallery/scripts/lib/run-key.mjs::worktreeKey`. Both hash the
/// absolute worktree-root path with the SAME FNV-1a, so a Rust build and a Node
/// harness in the same worktree agree on the key (asserted by a shared fixture
/// vector in both test suites).
pub fn worktree_key_for_cwd() -> String {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    worktree_key(&cwd)
}

/// Deterministic port BASE (search start) from a run key. The key seeds ONLY the
/// start — the caller then forward-searches with an OS bind-check for the first
/// actually-bindable port (the audit invariant: "no fixed port anywhere; the key
/// only seeds the search start"). `floor` is the lowest port of a harness's
/// disjoint range and `span` its width; the base is `floor + (key % span)`.
///
/// Byte-identical to the Node twin `run-key.mjs::portBase` (same modulo math over
/// the same 8-hex key parsed as u32), so a Rust caller and a Node caller in the
/// same worktree pick the same base.
pub fn port_base(key: &str, floor: u16, span: u16) -> u16 {
    // The key is 8 lowercase hex chars; parse as u32. A malformed key (should be
    // impossible from `worktree_key`) degrades to 0 → base == floor (still valid).
    let k = u32::from_str_radix(key, 16).unwrap_or(0);
    let span = span.max(1) as u32;
    floor.wrapping_add((k % span) as u16)
}

/// True when `DATABASE_URL` is the committed sentinel (or unset) AND the
/// dev hasn't opted out — i.e. it is safe to auto-isolate per worktree.
pub fn should_auto_isolate(database_url: &Option<String>) -> bool {
    if std::env::var("ZIEE_BUILD_DB_PERWORKTREE").as_deref() == Ok("0") {
        return false;
    }
    match database_url {
        // Unset → auto-isolate (the bare-default case).
        None => true,
        // Auto-isolate whenever we're pointed at the shared LOCAL build
        // cluster — that covers both the literal sentinel AND the
        // `ziee_build_<key>` URL the build rewrites itself to (which then
        // reaches the test harness at runtime via cargo:rustc-env). A
        // genuine operator/CI override to a DIFFERENT host:port is NOT the
        // local cluster, so it's honored unchanged (no suffix).
        Some(url) => url == DEFAULT_BUILD_DB_URL || url.contains(LOCAL_BUILD_CLUSTER),
    }
}

/// Replace the database/path component of a postgres URL, keeping the
/// scheme/userinfo/host/port. `postgresql://u:p@h:5432/postgres` +
/// `ziee_build_ab12cd34` → `postgresql://u:p@h:5432/ziee_build_ab12cd34`.
pub fn with_database(url: &str, db_name: &str) -> String {
    match url.rfind('/') {
        // Find the '/' that begins the path (after host:port). The last
        // '/' in a standard URL is the db path separator.
        Some(i) if i > url.find("://").map(|s| s + 2).unwrap_or(0) => {
            format!("{}/{}", &url[..i], db_name)
        }
        _ => format!("{}/{}", url.trim_end_matches('/'), db_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_is_stable_and_8_hex() {
        let a = worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/server");
        let b = worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/server");
        assert_eq!(a, b);
        assert_eq!(a.len(), 8);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn server_and_desktop_share_one_worktree_key() {
        let srv = worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/server");
        let dsk = worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/desktop/tauri");
        assert_eq!(srv, dsk, "same worktree → same key regardless of crate");
    }

    #[test]
    fn different_worktrees_differ() {
        let a = worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/server");
        let b = worktree_key("/data/pbya/ziee/tmp/xwt-b/src-app/server");
        assert_ne!(a, b);
    }

    #[test]
    fn cross_language_parity_fixture() {
        // SHARED FIXTURE VECTOR — the Node twin (run-key.test.mjs) pins the SAME
        // literal for the SAME worktree-root string. If either language's FNV
        // drifts, one of the two suites goes red. `worktree_key` strips /src-app,
        // so the manifest form and the bare-root form hash the identical string.
        assert_eq!(worktree_key("/data/pbya/ziee/tmp/xwt-a/src-app/server"), "12080097");
        assert_eq!(worktree_key("/data/pbya/ziee/tmp/xwt-a"), "12080097");
        assert_eq!(worktree_key("/data/pbya/ziee/tmp/xwt-b"), "1208014a");
    }

    #[test]
    fn port_base_is_deterministic_and_in_range() {
        let key = worktree_key("/data/pbya/ziee/tmp/xwt-a");
        let a = port_base(&key, 20000, 200);
        let b = port_base(&key, 20000, 200);
        assert_eq!(a, b, "same key + range → same base");
        assert!((20000..20200).contains(&a), "base within [floor, floor+span)");
        // Disjoint floors never collide for the same key.
        assert_ne!(port_base(&key, 20000, 200), port_base(&key, 22000, 200));
        // A malformed key degrades to floor, never panics.
        assert_eq!(port_base("zzzzzzzz", 9000, 200), 9000);
    }

    #[test]
    fn sentinel_detection() {
        assert!(should_auto_isolate(&None));
        assert!(should_auto_isolate(&Some(DEFAULT_BUILD_DB_URL.to_string())));
        // The build rewrites the URL to ziee_build_<key> on the same local
        // cluster — still auto-isolate (this is the value the harness sees at
        // runtime via cargo:rustc-env).
        assert!(should_auto_isolate(&Some(
            "postgresql://postgres:password@127.0.0.1:54321/ziee_build_958d7dd6".to_string()
        )));
        // A genuine external override is honored unchanged.
        assert!(!should_auto_isolate(&Some(
            "postgresql://u:p@db.prod:5432/real".to_string()
        )));
    }

    #[test]
    fn with_database_swaps_path() {
        assert_eq!(
            with_database("postgresql://postgres:password@127.0.0.1:54321/postgres", "ziee_build_ab12cd34"),
            "postgresql://postgres:password@127.0.0.1:54321/ziee_build_ab12cd34"
        );
    }

    #[test]
    fn with_database_appends_when_url_has_no_path() {
        // No db path after host:port → append one (don't corrupt host/port).
        assert_eq!(
            with_database("postgresql://postgres:password@127.0.0.1:54321", "mydb"),
            "postgresql://postgres:password@127.0.0.1:54321/mydb"
        );
    }

    #[test]
    fn with_database_handles_trailing_slash() {
        assert_eq!(
            with_database("postgresql://postgres:password@127.0.0.1:54321/", "mydb"),
            "postgresql://postgres:password@127.0.0.1:54321/mydb"
        );
    }

    #[test]
    fn with_database_admin_url_targets_maintenance_db() {
        // The `ensure_build_db` admin path builds a maintenance URL by swapping
        // in `postgres` — the swap must preserve userinfo + host + port.
        assert_eq!(
            with_database(
                "postgresql://u:p@db.example:6543/ziee_build_deadbeef",
                "postgres"
            ),
            "postgresql://u:p@db.example:6543/postgres"
        );
    }
}
