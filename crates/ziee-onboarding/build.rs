//! `ziee-onboarding` build script — provisions the **onboarding-only** build DB
//! for sqlx compile-time `query!` / `query_as!` verification.
//!
//! The crate's `query!` macros touch ONLY `user_onboarding`, so its own
//! `migrations/` dir (the table, with NO foreign keys) is a self-sufficient
//! standalone build DB — no other module's schema is needed. The shared
//! build-support provisioner names a per-worktree-isolated database
//! (`ziee_onboarding_build_<key>`) on the committed `:54321` cluster so
//! concurrent worktree builds don't clobber each other; a genuine external
//! `DATABASE_URL` override (a different host:port, as CI/production sets) is
//! honored unchanged.

fn main() {
    let migrations = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    // Sync fn: `provision_build_db` builds its own current-thread runtime.
    ziee_build_support::provision_build_db(&migrations);
}
