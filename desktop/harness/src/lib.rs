//! `ziee-desktop-harness` — the reusable desktop shell for an SDK app.
//!
//! Depends ONLY on the SDK crates (`ziee-core` / `ziee-identity` / `ziee-auth`),
//! never on `ziee`/the app, so a second app (CytoAnalyst) gets the desktop
//! experience for free.
//!
//! ## Present contents (Chunk D, clean parts)
//!
//! - [`manifest`] — the FOUR-part capability manifest keyed by deployment mode,
//!   replacing the four scattered mode-gating mechanisms (design-gate 1).
//! - [`single_user`] — the single-user auto-login strategy + owner-`*` model,
//!   selecting `ziee-auth`'s mint path + `ziee-identity`'s wildcard RBAC
//!   (design-gate 2).
//! - [`boot`] — the `ServerBoot` seam the app implements to hand the harness a
//!   booted server's `{addr, pool, jwt}`.
//!
//! ## Deferred to the BG-3 prerequisite (see `.extraction/D/STOP_REPORT.md`)
//!
//! The live Tauri shell (`run` / `run_headless`, `register_desktop_invoke_handler`,
//! the two IPC commands, per-OS `create_main_window`) and the embedded-Postgres
//! relocation into `ziee-framework`'s DB bootstrap MOVE here only after the
//! app-side `Repos`/JWT/config globals are threaded behind [`boot::ServerBoot`].
//! Moving that live boot path before the seam exists would force a broken tree
//! (the harness cannot reference `ziee::`), so it is reported, not forced.

pub mod boot;
pub mod manifest;
pub mod single_user;

pub use boot::{BootHandle, ServerBoot};
pub use manifest::{CapabilityManifest, DeploymentMode, FrontendManifest};
pub use single_user::{OwnerLogin, SingleUserStrategy, OWNER_WILDCARD_PERMISSION};
