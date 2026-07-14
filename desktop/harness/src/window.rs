//! The reusable desktop **window + boot→window orchestration** shell
//! (Chunk D-full).
//!
//! Moved verbatim from `ziee-desktop`'s `modules/backend/mod.rs`:
//!   - `create_main_window` (the per-OS main-window construction), and
//!   - the boot→window spawn skeleton of `start_backend_server`
//!     ([`spawn_boot_then_window`]), generalized over the [`ServerBoot`] seam.
//!
//! A second SDK app (CytoAnalyst) gets the identical window lifecycle for free.
//! The app supplies only its window parameters ([`WindowConfig`]) + its domain
//! post-boot steps (the `on_ready` closure); the harness owns the per-OS window
//! chrome, the boot spawn, and the "create the window on BOTH the success and
//! failure paths" contract. The harness names ONLY the [`ServerBoot`] seam —
//! never `ziee::` — so it stays app-agnostic.

use std::future::Future;
use std::sync::Arc;

use crate::boot::{BootHandle, ServerBoot};

/// App-tunable parameters for the main window.
///
/// The per-OS decorations / blur-effects / overlay-titlebar behavior is the
/// shared desktop-shell look and is intentionally NOT parameterized (a second
/// app gets the identical chrome); only the values an app legitimately varies
/// live here.
#[derive(Clone, Debug)]
pub struct WindowConfig {
    /// Window title (ziee passes `""` — the titlebar is drawn by the web UI).
    pub title: String,
    /// Initial inner size `(width, height)` in logical px.
    pub inner_size: (f64, f64),
    /// Minimum inner size `(width, height)` in logical px.
    pub min_inner_size: (f64, f64),
    /// Window-effects corner radius.
    pub effect_radius: f64,
    /// macOS traffic-light inset `(x, y)`.
    pub traffic_light_position: (f64, f64),
}

/// Spawn the embedded-server boot on the Tauri async runtime, then create the
/// main window. Generic over the app's [`ServerBoot`]; the app's domain
/// post-boot steps run in `on_ready` between a successful boot and window
/// creation.
///
/// The window is created on BOTH the success and failure paths (a failed boot
/// still shows the window so the UI's bootstrap retry loop can render its
/// "backend failed" state) — preserved verbatim from the ziee desktop shell.
pub fn spawn_boot_then_window<F, Fut>(
    boot: Arc<dyn ServerBoot>,
    app_handle: tauri::AppHandle,
    window: WindowConfig,
    on_ready: F,
) where
    F: FnOnce(BootHandle) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tauri::async_runtime::spawn(async move {
        match boot.boot().await {
            Ok(handle) => {
                tracing::info!("Backend server started successfully on {}", handle.addr);

                // App-domain post-boot steps (stash jwt/pool, migrations, owner
                // ensure, backfills, ready-flag) run here, threading the handle.
                on_ready(handle).await;

                // Create window now that server is ready
                create_main_window(&app_handle, &window);
            }
            Err(e) => {
                tracing::error!("Failed to start backend server: {}", e);
                // Propagate the failure to the user instead of leaving the app
                // invisible. The spawned start task has no caller to return the
                // error to, so without this the window (created only on the Ok
                // path below) never appears and a failed launch is silent.
                // Creating the window anyway lets the desktop UI's bootstrap
                // retry loop run, time out against the dead backend, and render
                // the 'failed' state ("Backend failed to start. Try restarting
                // Ziee.").
                create_main_window(&app_handle, &window);
            }
        }
    });
}

/// Create the main window with platform-specific customizations
pub fn create_main_window(app_handle: &tauri::AppHandle, config: &WindowConfig) {
    tracing::info!("Creating main window...");

    // macOS: no native decorations initially (overlay titlebar set below).
    #[cfg(target_os = "macos")]
    let mut main_window_builder = tauri::webview::WebviewWindowBuilder::new(
        app_handle,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(config.title.as_str())
    .inner_size(config.inner_size.0, config.inner_size.1)
    .min_inner_size(config.min_inner_size.0, config.min_inner_size.1)
    .resizable(true)
    .fullscreen(false)
    .decorations(false)
    .center()
    .effects(tauri::utils::config::WindowEffectsConfig {
        effects: vec![
            tauri::window::Effect::Mica,
            tauri::window::Effect::Acrylic,
            tauri::window::Effect::Blur,
        ],
        state: Some(tauri::window::EffectState::Active),
        radius: Some(config.effect_radius),
        color: None,
    });

    // Windows + Linux: native decorations.
    //  - Windows then overlays decorum's custom titlebar (preserves Aero
    //    snapping, snap layouts that a pure HTML titlebar can't replicate).
    //  - Linux relies entirely on the WM's server-side decorations
    //    (xfwm4 / Mutter / KWin) for border + shadow + close/min/max
    //    trio. XFCE / GNOME / KDE all default to right-aligned buttons,
    //    which matches the project's other platforms.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    let main_window_builder = tauri::webview::WebviewWindowBuilder::new(
        app_handle,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title(config.title.as_str())
    .inner_size(config.inner_size.0, config.inner_size.1)
    // Match macOS's `min_inner_size(400, 600)` — the previous 800-wide
    // floor blocked the sidebar's xs-mode (drawer / overlay) layout
    // path from ever rendering on Windows. The responsive UI already
    // handles widths down to 400.
    .min_inner_size(config.min_inner_size.0, config.min_inner_size.1)
    .resizable(true)
    .fullscreen(false)
    .decorations(true)
    .center()
    .effects(tauri::utils::config::WindowEffectsConfig {
        effects: vec![
            tauri::window::Effect::Mica,
            tauri::window::Effect::Acrylic,
            tauri::window::Effect::Blur,
        ],
        state: Some(tauri::window::EffectState::Active),
        radius: Some(config.effect_radius),
        color: None,
    });

    // macOS: overlay titlebar with native traffic light position (no glitch on resize)
    // x=20 matches the standard inset Apple uses in Notes / Finder /
    // Mail; the earlier x=12 sat too close to the window edge and
    // read as "tighter than other macOS apps".
    #[cfg(target_os = "macos")]
    {
        main_window_builder = main_window_builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .decorations(true)
            .traffic_light_position(tauri::LogicalPosition::new(
                config.traffic_light_position.0,
                config.traffic_light_position.1,
            ));
    }

    main_window_builder
        .build()
        .expect("failed to build the main application window");

    // Post-build: Windows overlay
    #[cfg(target_os = "windows")]
    {
        use tauri::Manager;
        use tauri_plugin_decorum::WebviewWindowExt;
        let main_window = app_handle.get_webview_window("main").unwrap();
        main_window.create_overlay_titlebar().unwrap();
    }

    tracing::info!("Main window created successfully");
}
