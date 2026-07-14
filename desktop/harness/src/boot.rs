//! The `ServerBoot` seam — the app-provided embed-server boundary the reusable
//! Tauri shell is built on (Chunk D, the BG-3 prerequisite target).
//!
//! ## Why a seam, not a move
//!
//! The desktop shell's embed-server glue calls
//! `ziee::start_server_with_routes`, which is the **app's entire server
//! assembly** (`setup_server`): it initializes the global `Repos`, builds the
//! app-specific module vec via `create_modules()`, installs the app's
//! `ZieeIdentityResolver` + `AuthContext`, and ingests the control-mcp catalog.
//! None of that is app-agnostic, so it cannot move into a reusable crate. The
//! harness therefore depends on an **injected** boot function the app supplies —
//! exactly the pluggable-identity / de-globalized posture BG established for the
//! auth module. Threading this seam from the app (de-globalizing the desktop
//! *consumer* surface: `Repos`, the JWT `OnceLock`, the config statics) is the
//! **BG-3 prerequisite** that must land before the live Tauri shell moves here.
//!
//! Once the app implements [`ServerBoot`], the harness owns: `run` / `run_headless`,
//! `register_desktop_invoke_handler`, the two Tauri commands (`get_server_port`,
//! `auto_login`), per-OS `create_main_window`, and the single-user auto-login
//! wiring — all generic over the installed `Arc<dyn ServerBoot>`.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use sqlx::PgPool;
use ziee_auth::auth::JwtService;

/// What the harness needs back from the app after the embedded server is up: the
/// bound address (published to the Tauri command + the remote-access forwarder),
/// the DB pool (for the single-user strategy's owner lookup + mint), and the JWT
/// service (stashed for the `auto_login` command).
pub struct BootHandle {
    /// The address the embedded Axum server actually bound.
    pub addr: SocketAddr,
    /// The server's DB pool (`Repos.pool()` on the app side today).
    pub pool: PgPool,
    /// The server's JWT service (stashed into the app's `JWT_SERVICE`
    /// `OnceLock` today; the harness holds it for the `auto_login` command).
    pub jwt: Arc<JwtService>,
}

/// The app-implemented embed-server boundary.
///
/// The concrete impl calls the app's `start_server_with_routes` with the
/// desktop route re-layering closure (CORS + `Extension(jwt)` re-applied to the
/// merged desktop routes, per axum's non-propagating `.merge()`), runs the
/// desktop migrations, ensures the single owner exists (via the app's
/// `create_admin_user` domain CRUD — kept app-side by BA), and returns the
/// [`BootHandle`]. It is `Send + Sync` so it can be installed behind an
/// `Arc<dyn ServerBoot>` the harness holds for the lifetime of the run.
#[async_trait]
pub trait ServerBoot: Send + Sync {
    /// Boot the embedded server and return its handle. Called once, from the
    /// Tauri `.setup` (GUI) or the headless entrypoint.
    async fn boot(&self) -> anyhow::Result<BootHandle>;

    /// Tear the embedded server down (the app's `cleanup_server` — stops the
    /// embedded Postgres + closes the pool). Called on window-close / Ctrl-C.
    async fn shutdown(&self);
}
