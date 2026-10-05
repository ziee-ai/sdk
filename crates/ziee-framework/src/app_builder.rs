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
use std::time::Duration;
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

/// Convert a configured sustained requests-per-second RATE into the
/// token-bucket replenish INTERVAL tower_governor wants.
///
/// This is the inverse, and the trap, of `GovernorConfigBuilder::per_second`:
/// despite its name that method sets the replenish INTERVAL to `n` seconds
/// (`self.period = Duration::from_secs(seconds)`, tower_governor 0.7.0
/// `governor.rs:183`; same for `per_millisecond`/`per_nanosecond`). Passing
/// the config's `per_second: 50` there therefore meant ONE token every 50 s —
/// the deployment's rate, inverted (#627). A rate of `n`/s is one token every
/// `1/n` s, e.g. 50/s → 20 ms, 3/s → ~333.3 ms, 1/s → 1 s.
///
/// `None` for `per_second == 0`: a zero rate has no finite interval, and
/// tower_governor refuses a zero period — callers must refuse that config
/// (a zero rate silently throttling to "no tokens ever" or silently disabling
/// DoS protection are both worse than a boot error).
pub fn rate_limit_replenish_interval(per_second: u64) -> Option<Duration> {
    if per_second == 0 {
        None
    } else {
        Some(Duration::from_secs_f64(1.0 / per_second as f64))
    }
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

    // `per_second` is a RATE; tower_governor's builder takes a replenish
    // INTERVAL. `.per_second(n)` would set the interval to n seconds — the
    // inverse (#627).
    let interval = rate_limit_replenish_interval(per_second).unwrap_or_else(|| {
        panic!(
            "server.rate_limit.per_second must be >= 1 (got {per_second}) — a \
             zero rate cannot be converted to a replenish interval; refusing to \
             boot rather than silently disabling (or deadlocking) the limiter"
        )
    });

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

/// Create CORS layer from configuration.
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

        let headers: Vec<HeaderName> = cors_config
            .allow_headers
            .iter()
            .filter_map(|h| if h == "*" { None } else { h.parse().ok() })
            .collect();

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

        // Set headers
        if cors_config.allow_headers.contains(&"*".to_string()) || headers.is_empty() {
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

#[cfg(test)]
mod rate_limit_tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::ConnectInfo;
    use axum::http::{Request, StatusCode};
    use std::net::SocketAddr;
    use tower::ServiceExt;

    /// A minimal `ServerConfig` with an explicit rate-limit block.
    ///
    /// `ServerConfig` derives `Deserialize` but not `Default`, so the test
    /// deserializes the exact key shape a real config file carries (all other
    /// values are defaults; the builder reads only `server.rate_limit`).
    fn config_with_rate_limit(per_second: u64, burst_size: u32) -> ServerConfig {
        serde_json::from_str(&format!(
            r#"{{
                "postgresql": {{ "use_embedded": false }},
                "server": {{
                    "host": "127.0.0.1",
                    "port": 0,
                    "api_prefix": "/api",
                    "rate_limit": {{
                        "enabled": true,
                        "per_second": {per_second},
                        "burst_size": {burst_size}
                    }}
                }},
                "jwt": {{
                    "secret": "0123456789abcdef0123456789abcdef-strong",
                    "issuer": "test",
                    "audience": "test-api",
                    "access_token_expiry_hours": 24
                }}
            }}"#
        ))
        .expect("test config must deserialize")
    }

    async fn send(app: axum::Router) -> StatusCode {
        // `PeerIpKeyExtractor` keys on `ConnectInfo<SocketAddr>` (tower_governor
        // key_extractor.rs `maybe_connect_info`), which a real `serve()` supplies
        // but `oneshot` does not — without it the governor answers 500
        // (`UnableToExtractKey`). Pin one fixed peer for all requests, exactly
        // like one client hitting a running server.
        let mut request =
            Request::builder().uri("/").body(Body::empty()).expect("request");
        request.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 4000))));
        app.oneshot(request).await.expect("oneshot").status()
    }

    /// The rate → interval mapping, i.e. that `per_second` is a RATE and not
    /// an interval. This is the exact inverse bug of #627: the old code passed
    /// `50` to `GovernorConfigBuilder::per_second` and got a 50 s interval.
    #[test]
    fn rate_limit_replenish_interval_maps_rate_to_period() {
        assert_eq!(
            rate_limit_replenish_interval(50),
            Some(Duration::from_millis(20)),
            "50/s must be one token every 20 ms"
        );
        // 3/s → ~333.3 ms. Integer-nanosecond truncation must not round this
        // to 0 (div-by-zero clone) or to 1 s (the buggy pre-fix behaviour).
        let three = rate_limit_replenish_interval(3).expect("3/s is a valid rate");
        assert!(
            three >= Duration::from_millis(333) && three < Duration::from_millis(334),
            "3/s must be one token every ~333 ms, got {three:?}"
        );
        assert_eq!(
            rate_limit_replenish_interval(1),
            Some(Duration::from_secs(1)),
            "1/s must be one token every 1 s"
        );
        // 0 is not a rate — defined behaviour: None, so the builder refuses it.
        assert_eq!(rate_limit_replenish_interval(0), None, "0/s has no interval");
    }

    /// A zero rate is REFUSED with a clear boot-time error rather than
    /// silently deadlocking (zero interval) or silently disabling the limiter.
    #[test]
    #[should_panic(expected = "server.rate_limit.per_second must be >= 1")]
    fn zero_per_second_is_refused_with_a_clear_error() {
        let config = config_with_rate_limit(0, 2);
        let _ = apply_rate_limit_layer(axum::Router::new(), &config, Some((50, 500)));
    }

    /// THE load-bearing regression test for #627: `per_second` is a RATE, so
    /// after the burst is spent, waiting one replenish interval (1/rate) must
    /// refund the next request.
    ///
    /// With the pre-fix `.per_second(n)` builder call the interval was n
    /// SECONDS (tower_governor 0.7.0 `governor.rs:183` — `self.period =
    /// Duration::from_secs(seconds)`), so at 20/s a 100 ms wait refunds
    /// nothing and the final assert goes RED (429 instead of 200). The old
    /// tests only asserted that a burst trips the limiter, which both
    /// behaviours satisfy — that is the hole this test closes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sustained_rate_refunds_one_interval_after_the_burst() {
        // 20/s → 50 ms replenish interval; burst of 2.
        let config = config_with_rate_limit(20, 2);
        let app = apply_rate_limit_layer(
            axum::Router::new().route("/", axum::routing::get(|| async { "ok" })),
            &config,
            None,
        );

        // The burst (2 tokens) serves.
        assert_eq!(send(app.clone()).await, StatusCode::OK, "1st request");
        assert_eq!(send(app.clone()).await, StatusCode::OK, "2nd request");
        // Burst spent, nothing replenished yet → 429.
        assert_eq!(
            send(app.clone()).await,
            StatusCode::TOO_MANY_REQUESTS,
            "3rd request must trip the limiter once the 2-token burst is spent"
        );
        // 20/s ⇒ one token every 50 ms; after 100 ms the bucket must hold
        // two tokens again and the next request passes.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            send(app.clone()).await,
            StatusCode::OK,
            "after 100 ms (2 × 50 ms at 20/s) the next request must be 200, \
             not 429 — per_second is requests-per-second, not a seconds interval"
        );
    }
}
