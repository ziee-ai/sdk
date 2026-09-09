//! Application builder — module discovery + router assembly + CORS/rate-limit
//! layers (moved from ziee's `core/app_builder.rs` in Chunk B2).
//!
//! `register_event_handlers` stays app-side (it constructs the domain-coupled
//! `EventBus`); everything else — module instantiation, the combined API router,
//! and the two config-driven middleware layers — is app-agnostic and lives here.
//! `create_cors_layer` / `apply_rate_limit_layer` take `&ServerConfig` (the
//! framework config); an app passes `&its_config`, which deref-coerces.

use aide::axum::ApiRouter;
use aide::openapi::OpenApi;
use axum::http::header::HeaderName;
use axum::http::Method;
use sqlx::PgPool;
use std::sync::Arc;
use tower_http::cors::{AllowOrigin, Any, CorsLayer};

use ziee_core::ServerConfig;

use crate::module_api::{AppModule, ModuleContext, MODULE_ENTRIES};

/// Create and initialize all application modules.
///
/// Modules are automatically discovered at link time using linkme distributed
/// slices. Each module registers itself using `#[distributed_slice(MODULE_ENTRIES)]`.
pub fn create_modules() -> Vec<Box<dyn AppModule>> {
    // Collect modules from distributed slice
    let mut entries: Vec<_> = MODULE_ENTRIES.iter().collect();

    // Sort by order (lower numbers first), then by NAME to break ties.
    //
    // The name tiebreak is load-bearing, not tidiness. `sort_by_key` is STABLE,
    // so without it two modules sharing an `order` keep the relative position
    // linkme gave them — and a distributed slice's element order comes from the
    // LINKER, which is stable for one binary but varies between builds. Orders
    // collide freely in practice (in ziee: 70, 80, 82, 85, 87, 88, 89, 90 … each
    // shared by 2-3 modules), so a rebuild could silently reorder module init.
    //
    // Two consequences, one visible and one latent:
    //   • VISIBLE: routes register in module order, so the emitted openapi.json
    //     path order changed from build to build. That made the committed spec
    //     impossible to keep in sync — merge-gate's regen-parity check (C3)
    //     could never pass, and every "fix" was a fresh regen that drifted again
    //     on the next build. Diagnosed as a stale artifact twice before the real
    //     cause was found.
    //   • LATENT: any module whose init depends on a same-order module having
    //     run first is a heisenbug that appears and disappears across rebuilds.
    //
    // Sorting by (order, name) makes the sequence a pure function of the source.
    entries.sort_by_key(|e| (e.order, e.name));

    // Instantiate modules using their constructors
    let modules: Vec<Box<dyn AppModule>> =
        entries.iter().map(|entry| (entry.constructor)()).collect();

    tracing::info!("Loaded {} modules in order:", modules.len());
    for entry in entries.iter() {
        tracing::debug!(
            "  - {} (order: {}) - {}",
            entry.name,
            entry.order,
            entry.description
        );
    }

    modules
}

/// Initialize all modules with the given context.
pub fn initialize_modules(
    modules: &mut [Box<dyn AppModule>],
    context: &ModuleContext,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    for module in modules.iter_mut() {
        module
            .init(context)
            .map_err(|e| format!("Failed to initialize module {}: {}", module.name(), e))?;
        tracing::info!("Initialized module: {}", module.name());
    }
    Ok(())
}

/// Build API router with all module routes.
pub fn build_api_router(
    modules: &[Box<dyn AppModule>],
    api_prefix: &str,
    pool: PgPool,
) -> (ApiRouter, OpenApi) {
    // Build combined router from all modules
    // Modules handle their own state requirements internally
    let mut combined_router = ApiRouter::new();
    for module in modules.iter() {
        combined_router = module.register_routes(combined_router);
    }

    // Provide the DB pool as a request extension. Several handlers
    // (the local-LLM proxy at /local-llm/v1/*, llm_model upload +
    // validate) extract `Extension<PgPool>` rather than reaching for
    // the global `Repos`; without this layer those routes 500 on a
    // missing-extension rejection before their body ever runs.
    let combined_router = combined_router.layer(axum::Extension(pool));

    // Create OpenAPI documentation. Closes 14-core F-24 (Info): adds
    // a `bearerAuth` security scheme so generated clients (and the
    // Redoc/Swagger UI rendering of the spec) know to send the JWT
    // as `Authorization: Bearer …`. Per-operation `security` arrays
    // are still up to individual handlers (most use `with_permission`
    // which already encodes the permission requirement).
    let mut api_doc = OpenApi::default();
    let mut components = api_doc.components.unwrap_or_default();
    components.security_schemes.insert(
        "bearerAuth".to_string(),
        aide::openapi::ReferenceOr::Item(aide::openapi::SecurityScheme::Http {
            scheme: "bearer".to_string(),
            bearer_format: Some("JWT".to_string()),
            description: Some(
                "JWT obtained from POST /auth/login or POST /auth/register, \
                 sent as `Authorization: Bearer <token>`."
                    .to_string(),
            ),
            extensions: Default::default(),
        }),
    );
    api_doc.components = Some(components);

    // Nest all routes under the api_prefix
    let api_router = ApiRouter::new().nest(api_prefix, combined_router);

    (api_router, api_doc)
}

/// Conditionally apply the global rate limiter (tower-governor).
///
/// Behavior, by `server.rate_limit`:
/// - `Some` with `enabled == false`  → no `GovernorLayer` (explicit opt-out).
/// - `Some` with `enabled == true`   → apply with its `per_second`/`burst_size`.
/// - `None` (block omitted)          → use `default_when_absent`:
///     - `Some((per_second, burst_size))` → apply that default (the standalone
///       web server passes `Some((50, 500))` so an un-configured deployment is
///       still protected).
///     - `None` → no limiter (the embedded/desktop path passes `None`: the
///       Tauri app serves only its own local webview over 127.0.0.1, has no
///       per-peer-IP attack surface, and the limiter would 429 legitimate
///       burst traffic — chat streams, SSE, multi-file uploads).
///
/// Called from BOTH `lib.rs::setup_server` and `main.rs::main` so the two stay
/// in sync. Why the `enabled` toggle exists: the built-in code_sandbox + memory
/// MCP servers are reached over loopback (`http://127.0.0.1`), so every internal
/// tool-call request shares the same `PeerIpKeyExtractor` bucket as real user
/// traffic. A rapid agent tool loop drains that bucket and the server starts
/// returning HTTP 429 to itself; raise the limits, or set `enabled: false` to
/// opt out entirely.
pub fn apply_rate_limit_layer(
    router: axum::Router,
    config: &ServerConfig,
    default_when_absent: Option<(u64, u32)>,
) -> axum::Router {
    let resolved = match config.server.rate_limit.as_ref() {
        Some(r) if !r.enabled => {
            tracing::warn!(
                "Rate limiting DISABLED via config (server.rate_limit.enabled=false) — \
                 no per-IP throttling is applied to any route. Safe only for trusted / \
                 non-public deployments."
            );
            return router;
        }
        Some(r) => Some((r.per_second, r.burst_size)),
        None => default_when_absent,
    };

    let (per_second, burst_size) = match resolved {
        Some(v) => v,
        // No config block and no caller default → skip the limiter entirely
        // (embedded/desktop path).
        None => return router,
    };

    let governor_conf = Arc::new(
        tower_governor::governor::GovernorConfigBuilder::default()
            .per_second(per_second)
            .burst_size(burst_size)
            .key_extractor(tower_governor::key_extractor::PeerIpKeyExtractor)
            .finish()
            .expect("Failed to build governor config"),
    );
    router.layer(tower_governor::GovernorLayer {
        config: governor_conf,
    })
}

/// The custom request headers the FRAMEWORK itself reads. They are unioned into
/// every explicit `allow_headers` list by [`create_cors_layer`] — see
/// [`create_cors_layer_with`] for why omission must not be expressible.
pub const FRAMEWORK_REQUIRED_REQUEST_HEADERS: &[&'static str] =
    &[crate::sync::extractor::SYNC_CONNECTION_HEADER];

/// Create CORS layer from configuration.
///
/// Equivalent to [`create_cors_layer_with`] with only the framework's own
/// required headers ([`FRAMEWORK_REQUIRED_REQUEST_HEADERS`]). An APP that reads
/// its own custom request headers must call `create_cors_layer_with` and pass
/// them, because the framework cannot know them.
///
/// Closes 14-core F-04 (High) at the level of "operator visibility":
/// any deployment booting with `Any/Any/Any` (either via wildcard
/// `*` in allow_origins, missing config, or empty list) gets a loud
/// `tracing::error!` at boot. Production deployments behind a
/// reverse proxy must set an explicit origin allowlist. We don't
/// hard-fail boot because dev/test environments legitimately need
/// permissive CORS; the loud log is enough to catch the misconfig
/// in `journalctl`/`docker logs` review.
pub fn create_cors_layer(config: &ServerConfig) -> CorsLayer {
    create_cors_layer_with(config, FRAMEWORK_REQUIRED_REQUEST_HEADERS)
}

/// Create the CORS layer, UNIONING `always_allow` into an explicit
/// `allow_headers` list.
///
/// ## Why a union, and not "just list them in the config"
///
/// A header the API itself defines and reads is not an operator preference: a
/// deployment in which it is refused at preflight is simply broken. Before this,
/// every explicit `allow_headers` list — the desktop app's, `dev.example.yaml`'s,
/// and every operator's — had to independently re-list such a header, and
/// forgetting one was **silent**: the server logs nothing, the browser refuses
/// the request before it is sent, and `fetch` rejects rather than returning a
/// status.
///
/// That is not hypothetical. `X-Chat-Stream-Connection-Id` was missing from the
/// desktop allowlist, so `PUT /api/chat/stream/subscription` never reached the
/// server, every chat-stream connection stayed scoped to no conversation, and
/// live assistant tokens were dropped at the registry while the reply persisted
/// normally — the user saw a spinner that only a page reload resolved. Its
/// sibling `X-Sync-Connection-Id` had already been added, with a comment
/// describing this exact failure; the chat header simply never followed.
///
/// So omission is no longer expressible. The `*`-wildcard and empty-list
/// branches already resolve to `Any` and are untouched (unioning into `Any` is a
/// no-op); only the explicit-list branch gains the headers, and only ones the
/// server already accepts at the handler — which grants a cross-origin caller
/// nothing a same-origin request could not already do. Origin and method
/// allowlisting are unaffected.
///
/// Duplicates are dropped case-insensitively, so a config that DOES list a
/// required header stays byte-equivalent (`HeaderName` is lowercase-normalised).
///
/// The element type is `&'static str` deliberately: it makes
/// "these are compile-time constants the SERVER defines" a property the type
/// system enforces — a config- or user-derived `String` cannot be passed without
/// deliberately leaking it — so no future caller can quietly reopen the
/// allow-list to arbitrary, externally-supplied header names.
pub fn create_cors_layer_with(
    config: &ServerConfig,
    always_allow: &[&'static str],
) -> CorsLayer {
    // Chunk sdk-batteries (P1): a permissive-CORS default is expected on a
    // loopback (local-dev) bind, so downgrade the loud `SECURITY:` ERROR to a
    // debug line there — it was scaring devs on every localhost boot. A public
    // (non-loopback) bind still gets the full ERROR so a real misconfig is caught
    // in `journalctl`/`docker logs`.
    let is_loopback = matches!(
        config.server.host.as_str(),
        "127.0.0.1" | "localhost" | "::1"
    );
    let permissive_warning = |reason: &str| {
        if is_loopback {
            tracing::debug!(
                "CORS is permissive ({}) on a loopback bind — expected in local \
                 dev. Set server.cors.allow_origins to an explicit allowlist for \
                 public deployments (see config/prod.example.yaml).",
                reason
            );
        } else {
            tracing::error!(
                "SECURITY: CORS is permissive ({}). Any origin can call \
                 the API and read non-credentialed responses. Set \
                 server.cors.allow_origins to an explicit allowlist for \
                 production deployments (see config/prod.example.yaml). \
                 Closes 14-core F-04.",
                reason
            );
        }
    };

    if let Some(ref cors_config) = config.server.cors {
        let origins: Vec<_> = cors_config
            .allow_origins
            .iter()
            .filter_map(|origin| {
                if origin == "*" {
                    None
                } else {
                    origin.parse::<axum::http::HeaderValue>().ok()
                }
            })
            .collect();

        let methods: Vec<Method> = cors_config
            .allow_methods
            .iter()
            .filter_map(|m| m.parse().ok())
            .collect();

        // Parse the configured list, then UNION the always-allow headers into it.
        // `HeaderName` lowercase-normalises on parse, so `contains` is already the
        // case-insensitive comparison we want and a config that spells a required
        // header differently does not produce a duplicate entry.
        let mut headers: Vec<HeaderName> = cors_config
            .allow_headers
            .iter()
            .filter_map(|h| if h == "*" { None } else { h.parse().ok() })
            .collect();
        // Whether the CONFIGURED list alone resolves to `Any` — captured BEFORE the
        // union. Deciding this on the merged list instead would be a real
        // regression: an empty `allow_headers` means "permissive" today, and after
        // a union it would silently become "only the required headers", i.e.
        // narrower than what the operator asked for.
        let config_headers_are_any =
            cors_config.allow_headers.contains(&"*".to_string()) || headers.is_empty();
        for required in always_allow {
            match required.parse::<HeaderName>() {
                Ok(name) if !headers.contains(&name) => headers.push(name),
                Ok(_) => {}
                Err(_) => tracing::error!(
                    "CORS: required request header {:?} is not a valid header name and \
                     was skipped — cross-origin requests carrying it will be refused \
                     at preflight",
                    required
                ),
            }
        }

        let mut layer = CorsLayer::new();

        // Set origins
        if cors_config.allow_origins.contains(&"*".to_string()) || origins.is_empty() {
            permissive_warning("allow_origins is empty or contains '*'");
            layer = layer.allow_origin(Any);
        } else {
            layer = layer.allow_origin(AllowOrigin::list(origins));
        }

        // Set methods
        if methods.is_empty() {
            layer = layer.allow_methods(Any);
        } else {
            layer = layer.allow_methods(methods);
        }

        // Set headers. `Any` already permits every required header, so the union
        // above is a no-op on this branch and is deliberately not consulted here.
        if config_headers_are_any {
            layer = layer.allow_headers(Any);
        } else {
            layer = layer.allow_headers(headers);
        }

        layer
    } else {
        // Default permissive CORS if not configured
        permissive_warning("no server.cors block in config");
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    }
}

/// Serve `router` on `listener` with the two things every app's boot path must
/// remember but the obvious `axum::serve(listener, app)` omits (chunk
/// sdk-batteries / P1, fixes G7):
///   1. `into_make_service_with_connect_info::<SocketAddr>()` — surfaces the TCP
///      peer address so `apply_rate_limit_layer`'s tower-governor
///      `PeerIpKeyExtractor` can read it. Without it, EVERY request returns
///      tower-governor's raw "Unable To Extract Key!" with no hint why.
///   2. graceful shutdown on Ctrl-C / SIGTERM.
///
/// Drop-in for a plain `axum::serve(listener, router).await`.
pub async fn serve(
    listener: tokio::net::TcpListener,
    router: axum::Router,
) -> std::io::Result<()> {
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
}

/// Resolve on Ctrl-C or SIGTERM. Graceful-with-warning: a container that strips
/// signal-handler installation logs + falls back to "never returns" rather than
/// crashing (mirrors ziee's own `shutdown_signal`).
async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(e) = tokio::signal::ctrl_c().await {
            tracing::warn!("Failed to install Ctrl+C handler: {}", e);
            std::future::pending::<()>().await;
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(e) => {
                tracing::warn!("Failed to install SIGTERM handler: {}", e);
                std::future::pending::<()>().await;
            }
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    tracing::info!("Shutdown signal received");
}

#[cfg(test)]
mod order_determinism_tests {
    use super::*;
    use crate::module_api::ModuleEntry;

    fn dummy() -> Box<dyn AppModule> {
        unreachable!("constructor is never invoked by these tests")
    }

    /// The tiebreak must be the NAME, not the incoming (link) order.
    ///
    /// This is the regression that motivated it: `sort_by_key(|e| e.order)` is
    /// stable, so two modules sharing an `order` kept whatever relative position
    /// the LINKER happened to give them — deterministic within one binary,
    /// different across builds. Feeding the same entries in two different
    /// incoming orders simulates exactly that, and asserts the output does not
    /// depend on it.
    #[test]
    fn same_order_modules_sort_by_name_regardless_of_link_order() {
        let a = ModuleEntry { name: "alpha", order: 80, description: "", constructor: dummy };
        let b = ModuleEntry { name: "bravo", order: 80, description: "", constructor: dummy };
        let c = ModuleEntry { name: "charlie", order: 10, description: "", constructor: dummy };

        let sorted = |mut v: Vec<&ModuleEntry>| {
            v.sort_by_key(|e| (e.order, e.name));
            v.into_iter().map(|e| e.name).collect::<Vec<_>>()
        };

        // Two different "link orders" of the same set.
        let one = sorted(vec![&a, &b, &c]);
        let two = sorted(vec![&b, &c, &a]);

        assert_eq!(one, two, "module order must not depend on link order");
        assert_eq!(one, vec!["charlie", "alpha", "bravo"], "order first, then name");
    }

    /// NEGATIVE CONTROL: the old key really was ambiguous. Without it, the test
    /// above could pass for the wrong reason (e.g. if the inputs happened to be
    /// pre-sorted) and would not prove the tiebreak does any work.
    #[test]
    fn the_old_order_only_key_was_link_order_dependent() {
        let a = ModuleEntry { name: "alpha", order: 80, description: "", constructor: dummy };
        let b = ModuleEntry { name: "bravo", order: 80, description: "", constructor: dummy };

        let old = |mut v: Vec<&ModuleEntry>| {
            v.sort_by_key(|e| e.order); // the pre-fix key
            v.into_iter().map(|e| e.name).collect::<Vec<_>>()
        };

        assert_ne!(
            old(vec![&a, &b]),
            old(vec![&b, &a]),
            "if these matched, order-only sorting would already be deterministic \
             and the (order, name) tiebreak would be pointless"
        );
    }
}

/// TEST-2 — `create_cors_layer_with` must make an omitted required header
/// UNEXPRESSIBLE, without widening anything else.
///
/// Driven through the real tower service with a real `OPTIONS` preflight, not by
/// inspecting the builder: the observable contract is what a browser is told, and
/// a browser reads only `Access-Control-Allow-Headers`. (`CorsLayer` exposes no
/// getters, so there is no shortcut that would still be evidence.)
#[cfg(test)]
mod cors_required_headers_tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
        routing::put,
        Router,
    };
    use tower::ServiceExt; // oneshot

    const CHAT_HEADER: &str = "X-Chat-Stream-Connection-Id";

    /// Minimal `ServerConfig` carrying just the CORS block under test.
    fn config_with(allow_headers: &[&str]) -> ServerConfig {
        serde_json::from_value(serde_json::json!({
            "postgresql": { "use_embedded": false },
            "server": {
                "host": "127.0.0.1",
                "port": 8080,
                "api_prefix": "/api",
                "cors": {
                    "allow_origins": ["tauri://localhost"],
                    "allow_methods": ["GET", "PUT", "OPTIONS"],
                    "allow_headers": allow_headers,
                },
            },
            "jwt": {
                "secret": "test-secret-not-used-by-the-cors-layer",
                "issuer": "ziee",
                "audience": "ziee-api",
                "access_token_expiry_hours": 1,
            },
        }))
        .expect("minimal ServerConfig fixture must deserialize")
    }

    /// The `Access-Control-Allow-Headers` a browser would be told, for a preflight
    /// that asks to send `requested`.
    async fn preflight_allow_headers(layer: CorsLayer, requested: &str) -> Option<String> {
        let app = Router::new()
            .route("/api/chat/stream/subscription", put(|| async { StatusCode::NO_CONTENT }))
            .layer(layer);
        let res = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/chat/stream/subscription")
                    .header("Origin", "tauri://localhost")
                    .header("Access-Control-Request-Method", "PUT")
                    .header("Access-Control-Request-Headers", requested)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("preflight must produce a response");
        res.headers()
            .get("access-control-allow-headers")
            .and_then(|v| v.to_str().ok())
            .map(str::to_ascii_lowercase)
    }

    /// The defect itself: a config that does NOT list the header must still allow
    /// it. This is the assertion that goes red without the union.
    #[tokio::test]
    async fn union_allows_a_required_header_the_config_omits() {
        let cfg = config_with(&["Authorization", "Content-Type"]);
        let allowed = preflight_allow_headers(
            create_cors_layer_with(&cfg, &[CHAT_HEADER]),
            "content-type,x-chat-stream-connection-id",
        )
        .await
        .expect("an explicit allow-list must echo an Access-Control-Allow-Headers");
        assert!(
            allowed.contains("x-chat-stream-connection-id"),
            "a header the API reads must survive preflight even when the config \
             forgets it; got {allowed:?}"
        );
        // …and the operator's own entries are preserved, not replaced.
        assert!(allowed.contains("authorization"), "got {allowed:?}");
        assert!(allowed.contains("content-type"), "got {allowed:?}");
    }

    /// A config that DOES list it (in another case) must not gain a duplicate —
    /// `HeaderName` lowercase-normalises, so the union is a no-op there.
    #[tokio::test]
    async fn union_does_not_duplicate_a_differently_cased_entry() {
        let cfg = config_with(&["Authorization", "x-chat-STREAM-connection-id"]);
        let allowed =
            preflight_allow_headers(create_cors_layer_with(&cfg, &[CHAT_HEADER]), CHAT_HEADER)
                .await
                .expect("explicit allow-list");
        assert_eq!(
            allowed.matches("x-chat-stream-connection-id").count(),
            1,
            "the union must be case-insensitive; got {allowed:?}"
        );
    }

    /// The two permissive branches are untouched: `*` and an empty list both mean
    /// `Any`, which already permits every required header. Deciding this on the
    /// MERGED list would silently narrow an empty list to "only the required
    /// headers" — a regression, not a fix.
    #[tokio::test]
    async fn wildcard_and_empty_list_still_mean_any() {
        for allow in [vec!["*"], vec![]] {
            let cfg = config_with(&allow);
            let allowed = preflight_allow_headers(
                create_cors_layer_with(&cfg, &[CHAT_HEADER]),
                "x-anything-at-all",
            )
            .await
            .expect("Any still answers the preflight");
            assert!(
                allowed.contains('*') || allowed.contains("x-anything-at-all"),
                "allow_headers={allow:?} must stay permissive; got {allowed:?}"
            );
        }
    }

    /// An invalid entry is skipped with a loud log rather than panicking the boot.
    #[tokio::test]
    async fn an_invalid_required_header_does_not_panic_boot() {
        let cfg = config_with(&["Authorization"]);
        let allowed = preflight_allow_headers(
            create_cors_layer_with(&cfg, &["not a valid header", CHAT_HEADER]),
            CHAT_HEADER,
        )
        .await
        .expect("explicit allow-list");
        assert!(allowed.contains("x-chat-stream-connection-id"), "got {allowed:?}");
    }
}
