//! `skeleton-server` — the permanent, app-agnostic second-consumer guard.
//!
//! A minimal SDK-only server that must keep compiling AND booting against the
//! framework with no app-specific code, proving the platform layer stays
//! app-agnostic (plan §4, gate E11).
//!
//! It links ONLY `ziee-core` + `ziee-framework` (the SDK platform crates) plus
//! third-party crates — and NO ziee domain crate (chat / memory / mcp / …). It
//! registers exactly one module (`skeleton`) exposing one route (`GET
//! /api/ping` → `"pong"`) via the real framework machinery: a `ModuleEntry` in
//! the `MODULE_ENTRIES` distributed slice, `create_modules()`,
//! `initialize_modules()`, and `build_api_router()`. Then it boots on an
//! ephemeral loopback port, self-requests `/api/ping`, asserts the body is
//! `"pong"`, prints `SKELETON OK`, and exits 0.
//!
//! The DB pool is created **lazily** and never connected — the framework's
//! router assembly must not require a live database (build-DB-free, plan §7).

use std::sync::Arc;
use std::time::Duration;

use aide::axum::ApiRouter;
use axum::routing::get;
use linkme::distributed_slice;
use sqlx::postgres::PgPoolOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use ziee_core::config::{
    ExternalPostgreSqlConfig, HttpServerConfig, JwtConfig, PostgreSqlConfig, ServerConfig,
};
use ziee_framework::{
    app_builder::{build_api_router, create_modules, initialize_modules},
    AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES,
};

// --------------------------------------------------------------------------
// The one trivial module. Implements the framework `AppModule` trait; adds a
// single `GET /ping` route (nested under the `/api` prefix by build_api_router).
// --------------------------------------------------------------------------

struct SkeletonModule;

impl AppModule for SkeletonModule {
    fn name(&self) -> &'static str {
        "skeleton"
    }

    fn description(&self) -> &'static str {
        "Minimal app-agnostic second-consumer module (SDK boundary guard)."
    }

    fn init(&mut self, _ctx: &ModuleContext) -> Result<(), Box<dyn std::error::Error>> {
        // No DB, no state — the boundary guard stays trivial on purpose.
        Ok(())
    }

    fn register_routes(&self, router: ApiRouter) -> ApiRouter {
        router.route("/ping", get(ping))
    }
}

async fn ping() -> &'static str {
    "pong"
}

// Register the module at link time via the framework's distributed slice —
// exactly the same mechanism every real ziee module uses.
#[distributed_slice(MODULE_ENTRIES)]
static SKELETON: ModuleEntry = ModuleEntry {
    name: "skeleton",
    order: 10,
    description: "Minimal app-agnostic second-consumer module (SDK boundary guard).",
    constructor: || Box::new(SkeletonModule),
};

// --------------------------------------------------------------------------
// A minimal, valid `ServerConfig` built entirely in code (no YAML needed). The
// postgres coordinates point at a bogus loopback address that is never dialed —
// the pool is created with `connect_lazy`.
// --------------------------------------------------------------------------

fn minimal_config(port: u16) -> ServerConfig {
    ServerConfig {
        postgresql: PostgreSqlConfig {
            use_embedded: false,
            embedded: None,
            external: Some(ExternalPostgreSqlConfig {
                host: "127.0.0.1".to_string(),
                // Port 1 is never bound — proves the pool is lazy (never dialed).
                port: 1,
                username: "skeleton".to_string(),
                password: "skeleton".to_string(),
                database: "skeleton".to_string(),
            }),
            pool: None,
        },
        server: HttpServerConfig {
            host: "127.0.0.1".to_string(),
            port,
            api_prefix: "/api".to_string(),
            cors: None,
            rate_limit: None,
            trust_forwarded_headers: false,
            max_file_upload_mb: 128,
        },
        logging: None,
        jwt: JwtConfig {
            secret: "skeleton-example-not-a-real-secret".to_string(),
            issuer: "skeleton".to_string(),
            audience: "skeleton-api".to_string(),
            access_token_expiry_hours: 24,
            refresh_token_expiry_days: 30,
            access_token_expiry_seconds: None,
        },
    }
}

#[tokio::main]
async fn main() {
    // Global watchdog: this self-test must never hang. If anything blocks,
    // exit non-zero after 20s so CI fails loudly instead of stalling.
    tokio::spawn(async {
        tokio::time::sleep(Duration::from_secs(20)).await;
        eprintln!("SKELETON TIMEOUT: self-test did not complete within 20s");
        std::process::exit(1);
    });

    let config = Arc::new(minimal_config(0));

    // A LAZY, never-connected pool. `connect_lazy` only validates the URL; it
    // does NOT dial the (nonexistent) database. Router assembly must succeed
    // with no live DB.
    let pool = PgPoolOptions::new()
        .connect_lazy(&config.database_url())
        .expect("connect_lazy should accept a well-formed URL");

    // Build the module context the same way the real app does. `app_config`
    // is the opaque app-config slot — the skeleton has no monolithic config,
    // so it injects a trivial `()`.
    let ctx = ModuleContext::new(
        Arc::new(pool.clone()),
        config.clone(),
        Arc::new(()) as Arc<dyn std::any::Any + Send + Sync>,
    );

    // Discover + init modules via the real framework machinery.
    let mut modules = create_modules();
    assert!(
        modules.iter().any(|m| m.name() == "skeleton"),
        "the skeleton module must be discovered from MODULE_ENTRIES"
    );
    initialize_modules(&mut modules, &ctx).expect("module init should succeed");

    // Assemble the combined API router (routes nested under /api).
    let (api_router, mut openapi) =
        build_api_router(&modules, &config.server.api_prefix, pool.clone());
    let app: axum::Router = api_router.finish_api(&mut openapi);

    // Bind an ephemeral loopback port and serve in the background.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            eprintln!("SKELETON SERVE ERROR: {e}");
            std::process::exit(1);
        }
    });

    // Self-request GET /api/ping and assert the body is exactly "pong".
    let body = http_get(addr, "/api/ping")
        .await
        .expect("GET /api/ping should return a response");

    assert_eq!(
        body, "pong",
        "GET /api/ping body must be \"pong\", got {body:?}"
    );

    println!("SKELETON OK");
    std::process::exit(0);
}

/// Minimal raw-HTTP/1.1 GET returning the response body as a String. Avoids
/// pulling an HTTP client dependency — the boundary guard stays lean.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(addr)
        .await
        .map_err(|e| format!("connect: {e}"))?;

    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n",
        addr
    );
    stream
        .write_all(req.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;

    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .await
        .map_err(|e| format!("read: {e}"))?;

    let text = String::from_utf8_lossy(&raw);
    let status_ok = text
        .lines()
        .next()
        .map(|l| l.contains(" 200 "))
        .unwrap_or(false);
    if !status_ok {
        return Err(format!("non-200 status line: {:?}", text.lines().next()));
    }

    // Split headers from body on the blank line.
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .ok_or_else(|| "no header/body separator in response".to_string())?;

    // With `Connection: close` and a small body, axum sends it either
    // Content-Length'd or chunked. For a 4-byte body it's Content-Length, so
    // the body is exactly "pong". Trim any trailing chunk framing just in case.
    Ok(body.trim().to_string())
}
