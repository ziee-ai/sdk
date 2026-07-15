//! When built with the `routes` feature (which the `module` feature implies),
//! provision + migrate a per-worktree build DB from this crate's own migration
//! and set `DATABASE_URL` (scoped to this crate) so the `query!`/`query_as!`
//! macros in `repository` verify SQL against the real `notifications` schema at
//! `cargo check` time. A types-only build (no `routes`) skips this entirely and
//! needs no Postgres — the crate's build-DB-free property is preserved for
//! consumers that only want the wire types.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=migrations");
    // `CARGO_FEATURE_ROUTES` is set by cargo iff the `routes` feature is enabled
    // (the `module` feature enables `routes`, so it is set for that too). The
    // compile-time `query!` macros live in the `routes`-gated `repository`.
    if std::env::var_os("CARGO_FEATURE_ROUTES").is_none() {
        return;
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    ziee_build_support::provision_build_db(&manifest.join("migrations"));
}
