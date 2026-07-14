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
//! ## Window + boot orchestration shell (Chunk D-full)
//!
//! - [`window`] — the per-OS main-window construction (`create_main_window`) +
//!   the boot→window spawn skeleton ([`window::spawn_boot_then_window`]),
//!   generic over [`boot::ServerBoot`]. Moved verbatim from the ziee desktop
//!   shell; the app supplies its [`window::WindowConfig`] + its domain post-boot
//!   closure and gets the identical window lifecycle for free.
//!
//! ## Kept app-side (fundamentally app-domain — see `.extraction/D-full/CUT.md`)
//!
//! `run` / `run_headless` wrap the app's whole module system + `ziee::Config`
//! assembly (`start_server_with_routes` = "the app's entire server assembly",
//! per [`boot`]); `register_desktop_invoke_handler` + the two IPC commands
//! (`get_server_port`, `auto_login`) resolve the in-crate `#[tauri::command]`
//! macros + reach app-domain repositories. None can move without the harness
//! naming `ziee::`, so they stay in `ziee-desktop` as thin consumers of this
//! crate's window/boot shell + the [`boot::ServerBoot`] seam.

pub mod boot;
pub mod manifest;
pub mod single_user;
pub mod window;

pub use boot::{BootHandle, ServerBoot};
pub use manifest::{CapabilityManifest, DeploymentMode, FrontendManifest};
pub use single_user::{OwnerLogin, SingleUserStrategy, OWNER_WILDCARD_PERMISSION};
pub use window::{create_main_window, spawn_boot_then_window, WindowConfig};
