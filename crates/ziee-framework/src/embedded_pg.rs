//! Generic embedded / external Postgres bootstrap (Chunk BG-3).
//!
//! The runtime database-bring-up: build an embedded `postgresql_embedded`
//! instance from the app-agnostic [`PostgreSqlConfig`] (or connect to an
//! external URL), run the app's migrations, keep the instance alive for the
//! process lifetime, and stop it cleanly on shutdown / panic / drop.
//!
//! This is **app-agnostic** — it names no schema, holds no `query!`
//! (build-DB-free), and is **parameterized** over:
//!   - the app's merged [`Migrator`] (`&'static`, so the app owns its
//!     schema-bound `sqlx::migrate!` + any `set_ignore_missing`),
//!   - the Postgres binary version used for the `pg_ctl` stop path
//!     (`pg_ctl_version`, an app build-env constant — the SDK workspace has no
//!     `ZIEE_POSTGRES_VERSION` env, so it can't `env!` it),
//!   - two [`EmbeddedPgHooks`] callbacks the app fills to install + smoke-test
//!     its Postgres extensions (ziee installs pgvector for the memory module).
//!
//! Extracted verbatim (behaviour-preserving) from ziee's
//! `server/core/database/mod.rs`; ziee's `core::database` is now a thin
//! orchestration shim that passes its migrator + pgvector hooks in.

use postgresql_embedded::{PostgreSQL, Settings, VersionReq};
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Command;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::OnceCell;
use ziee_core::config::PostgreSqlConfig;

static DATABASE_POOL: OnceCell<Arc<PgPool>> = OnceCell::const_new();
static POSTGRESQL_INSTANCE: OnceCell<Arc<Mutex<PostgreSQL>>> = OnceCell::const_new();
static CLEANUP_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Default embedded-Postgres subdir under the app data dir
/// (`ziee_core::app_state::get_app_data_dir()` = `~/.<app_name>` unless the app
/// set it via `set_app_data_dir`/`set_app_name`), matching ziee's config
/// `resolve_paths` convention (`<app.data_dir>/postgres` + `/postgres-data`).
/// Used when an app leaves `postgresql.embedded.{installation_dir,data_dir}`
/// unset, so no ziee-side `Config::resolve_paths` is required to boot.
fn default_embedded_dir(sub: &str) -> String {
    ziee_core::app_state::get_app_data_dir()
        .join(sub)
        .to_string_lossy()
        .into_owned()
}

/// Fill unset `installation_dir` / `data_dir` on an embedded-PG config with the
/// app-data-dir-derived defaults (`<data dir>/postgres` + `/postgres-data`), so
/// an app can resolve them ONCE instead of hard-coding both paths in YAML. A
/// no-op for fields already set (operator/app overrides win). Optional — the
/// embedded bring-up applies the same defaults inline — but handy for an app
/// that wants the resolved paths visible on its own config struct before boot.
pub fn resolve_embedded_paths(cfg: &mut ziee_core::config::EmbeddedPostgreSqlConfig) {
    cfg.installation_dir
        .get_or_insert_with(|| default_embedded_dir("postgres"));
    cfg.data_dir
        .get_or_insert_with(|| default_embedded_dir("postgres-data"));
}

/// App-supplied Postgres-extension hooks, threaded through the generic embedded
/// bring-up so the framework installs no app-specific extension itself. Both are
/// plain `fn` pointers (no captured state) so they survive the multi-attempt
/// init retry. ziee fills them with the pgvector install + `CREATE EXTENSION`
/// smoke-test that the memory module depends on.
#[derive(Clone, Copy)]
pub struct EmbeddedPgHooks {
    /// Called AFTER `PostgreSQL::setup()` and BEFORE `start()`, with the
    /// installation dir — Postgres only scans `share/extension/` at boot, so an
    /// extension must be installed into the installation dir before start.
    pub after_setup: fn(&Path),
    /// Called AFTER `start()`, with the `postgres` database URL. Runs the
    /// extension smoke-test (e.g. `CREATE EXTENSION IF NOT EXISTS vector`) and
    /// records availability. Returns a boxed future the bring-up awaits.
    pub smoke_test: fn(String) -> Pin<Box<dyn Future<Output = ()> + Send>>,
}

/// Stop any running PostgreSQL instance by checking for postmaster.pid and using pg_ctl stop
fn stop_existing_postgres_instance(
    installation_dir: &PathBuf,
    pg_ctl_version: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let data_dir = installation_dir.join("data");
    let postmaster_pid_path = data_dir.join("postmaster.pid");

    if !postmaster_pid_path.exists() {
        println!("No postmaster.pid found, no existing PostgreSQL instance to stop");
        return Ok(());
    }

    println!("Found existing postmaster.pid, stopping PostgreSQL instance...");

    // Handle cross-platform executable naming
    let pg_ctl_exe = if cfg!(target_os = "windows") {
        "pg_ctl.exe"
    } else {
        "pg_ctl"
    };

    let pg_ctl_path = installation_dir
        .join(pg_ctl_version)
        .join("bin")
        .join(pg_ctl_exe);

    // Check if pg_ctl executable exists
    if !pg_ctl_path.exists() {
        println!("Warning: pg_ctl executable not found at {:?}", pg_ctl_path);
        return Ok(());
    }

    let output = Command::new(&pg_ctl_path)
        .arg("stop")
        .arg("-D")
        .arg(&data_dir)
        .arg("-m")
        .arg("fast") // Use fast shutdown mode
        .output()?;

    if output.status.success() {
        println!("Successfully stopped existing PostgreSQL instance");
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!(
            "Error: Failed to stop PostgreSQL instance. Exiting to prevent database corruption."
        );
        eprintln!("STDERR: {}", stderr);
        eprintln!("STDOUT: {}", stdout);
        std::process::exit(1);
    }

    // Wait a moment for the process to fully stop
    std::thread::sleep(std::time::Duration::from_millis(1000));

    Ok(())
}

/// Initialize (once) the process-wide database pool: bring up embedded Postgres
/// or connect externally, run `migrator`, and cache the pool. `pg` /
/// `external_url` come from the app's `ServerConfig`; `pg_ctl_version` is the
/// app's Postgres binary version constant; `hooks` install the app's extensions.
pub async fn initialize_database(
    pg: PostgreSqlConfig,
    external_url: String,
    pg_ctl_version: String,
    migrator: &'static Migrator,
    hooks: EmbeddedPgHooks,
) -> Result<Arc<PgPool>, Box<dyn std::error::Error + Send + Sync>> {
    println!("Initializing database");

    let pool = DATABASE_POOL
        .get_or_try_init(|| async move {
            // Retry logic for database initialization
            let max_retries = 5;
            let retry_delay = std::time::Duration::from_secs(3);

            for attempt in 1..=max_retries {
                println!(
                    "Database initialization attempt {} of {}",
                    attempt, max_retries
                );

                match try_initialize_database_once(
                    &pg,
                    &external_url,
                    &pg_ctl_version,
                    migrator,
                    hooks,
                )
                .await
                {
                    Ok(pool) => {
                        return Ok::<Arc<PgPool>, Box<dyn std::error::Error + Send + Sync>>(pool);
                    }
                    Err(e) => {
                        eprintln!("Database initialization attempt {} failed: {}", attempt, e);
                        if attempt < max_retries {
                            println!("Waiting {} seconds before retry...", retry_delay.as_secs());
                            tokio::time::sleep(retry_delay).await;
                        } else {
                            return Err(format!(
                                "Failed to initialize database after {} attempts: {}",
                                max_retries, e
                            )
                            .into());
                        }
                    }
                }
            }

            unreachable!()
        })
        .await?;

    //test query again to ensure the connection is valid after migrations
    let new_pool = get_database_pool()?;
    sqlx::query("SELECT 1").execute(new_pool.as_ref()).await?;

    println!("Database initialized successfully");

    Ok(pool.clone())
}

async fn try_initialize_database_once(
    pg: &PostgreSqlConfig,
    external_url: &str,
    pg_ctl_version: &str,
    migrator: &'static Migrator,
    hooks: EmbeddedPgHooks,
) -> Result<Arc<PgPool>, Box<dyn std::error::Error + Send + Sync>> {
    let database_url = if pg.use_embedded {
        // Initialize embedded PostgreSQL
        let embedded = pg
            .embedded
            .as_ref()
            .ok_or("embedded config must be present when use_embedded is true")?;

        let mut settings = Settings::default();
        settings.version = VersionReq::parse(&format!("={}", embedded.version))?;
        settings.temporary = false;

        // installation_dir / data_dir default to `<app data dir>/postgres` and
        // `<app data dir>/postgres-data` when unset — the same convention ziee's
        // config path-resolution uses — so a fresh app need NOT hard-code both
        // in YAML. ziee always fills these before boot (its `resolve_paths`), so
        // its `Some(...)` values win here and the behaviour is byte-identical;
        // the app-data-dir defaults only fire for an app that left them unset.
        // (Scrubbed the former `Config::resolve_paths` panic message — that is a
        // ziee-app-side symbol absent from the SDK.)
        let installation_dir = embedded
            .installation_dir
            .clone()
            .unwrap_or_else(|| default_embedded_dir("postgres"));
        let data_dir_str = embedded
            .data_dir
            .clone()
            .unwrap_or_else(|| default_embedded_dir("postgres-data"));
        settings.installation_dir = PathBuf::from(&installation_dir);

        // Stop any existing PostgreSQL instance before proceeding
        stop_existing_postgres_instance(&settings.installation_dir, pg_ctl_version)?;

        settings.username = embedded.username.clone();
        settings.password_file = settings.installation_dir.join(".pgpass");
        if settings.password_file.exists() {
            settings.password = std::fs::read_to_string(settings.password_file.clone())?;
        } else {
            settings.password = embedded.password.clone();
        }
        settings.data_dir = PathBuf::from(&data_dir_str);

        // Set timezone from config
        settings
            .configuration
            .insert("timezone".to_string(), embedded.timezone.clone());
        settings
            .configuration
            .insert("log_timezone".to_string(), embedded.log_timezone.clone());

        // Use port and bind address from config
        settings.port = embedded.port;
        settings.host = embedded.bind_address.clone();

        // Set logging configuration from config
        let logging_collector = if embedded.logging.collector {
            "on"
        } else {
            "off"
        };
        settings.configuration.insert(
            "logging_collector".to_string(),
            logging_collector.to_string(),
        );
        settings.configuration.insert(
            "log_directory".to_string(),
            embedded.logging.directory.clone(),
        );
        settings.configuration.insert(
            "log_filename".to_string(),
            embedded.logging.filename.clone(),
        );
        settings.configuration.insert(
            "log_statement".to_string(),
            embedded.logging.statement.clone(),
        );

        let mut postgresql = PostgreSQL::new(settings);
        println!(
            "Setting up embedded PostgreSQL at port {}",
            postgresql.settings().port
        );

        postgresql.setup().await?;

        // Install the app's Postgres extension(s) into the embedded-PG
        // installation dir BEFORE start() — Postgres only scans
        // `share/extension/` at boot for CREATE EXTENSION lookups. The app's
        // `after_setup` hook is fail-soft internally (ziee's installs pgvector
        // and logs + continues if the build embedded zero-byte stubs).
        (hooks.after_setup)(&postgresql.settings().installation_dir);

        println!("Starting embedded PostgreSQL...");
        postgresql.start().await?;

        // Run the app's extension smoke-test (ziee: CREATE EXTENSION vector +
        // mark_available so the memory module knows it can use vector(N)).
        let smoke_url = postgresql.settings().url("postgres");
        (hooks.smoke_test)(smoke_url).await;

        let database_url = postgresql.settings().url("postgres");
        // Log only the host:port + db name; the embedded URL contains
        // the auto-generated password. Closes 14-core F-12 (Medium).
        match url::Url::parse(&database_url) {
            Ok(u) => println!(
                "Embedded PostgreSQL ready: {}://{}:{}{}",
                u.scheme(),
                u.host_str().unwrap_or("?"),
                u.port().map(|p| p.to_string()).unwrap_or_else(|| "?".to_string()),
                u.path()
            ),
            Err(_) => println!("Embedded PostgreSQL ready (URL not loggable)"),
        }

        // Store the PostgreSQL instance to keep it alive
        POSTGRESQL_INSTANCE
            .set(Arc::new(Mutex::new(postgresql)))
            .map_err(|_| "Failed to store PostgreSQL instance")?;

        // Register cleanup handlers once
        register_cleanup_handlers();

        // Initialize the static cleanup instance
        std::sync::LazyLock::force(&_CLEANUP);

        database_url
    } else {
        // Use external PostgreSQL
        let external = pg
            .external
            .as_ref()
            .ok_or("external config must be present when use_embedded is false")?;
        println!(
            "Using external PostgreSQL at {}:{}",
            external.host, external.port
        );
        external_url.to_string()
    };

    // Wait for PostgreSQL to be ready with retry logic
    let pool = connect_with_retry(&database_url, pg).await?;

    //test query to ensure the connection is valid
    println!("Testing database connection...");
    sqlx::query("SELECT 1").execute(&pool).await?;

    // Run migrations.
    //
    // The app passes a `&'static Migrator` with `set_ignore_missing(true)`
    // already applied — sqlx must NOT panic when the _sqlx_migrations table
    // contains entries this binary doesn't recognise (those are the desktop
    // app's own migrations applied against the shared DB — see
    // src-app/desktop/). It does NOT apply external/untrusted migrations. The
    // desktop + server share `_sqlx_migrations` and each binary owns its own
    // subset; ignore_missing is the supported sqlx pattern for that setup.
    // Chunk BA-full: the app's migrator is the MERGED migration set (the app's
    // own `migrations/` ∪ `ziee-auth`'s structural auth-table migrations,
    // composed by build.rs, version-sorted). Reproduces ziee's exact
    // `_sqlx_migrations` history, so existing deployments are unaffected.
    println!("Running database migrations...");
    migrator.run(&pool).await?;

    Ok(Arc::new(pool))
}

async fn connect_with_retry(
    database_url: &str,
    pg: &PostgreSqlConfig,
) -> Result<PgPool, Box<dyn std::error::Error + Send + Sync>> {
    use sqlx::postgres::PgPoolOptions;
    use std::time::Duration;

    let max_retries = 10;
    let mut retry_count = 0;

    println!("Attempting to connect to database with retry logic...");

    // Configure connection pool with timeouts from config or defaults
    let pool_config = pg.pool.as_ref();
    let max_connections = pool_config.map(|p| p.max_connections).unwrap_or(10);
    let min_connections = pool_config.map(|p| p.min_connections).unwrap_or(1);
    let acquire_timeout_secs = pool_config.map(|p| p.acquire_timeout_secs).unwrap_or(5);

    let mut pool_options = PgPoolOptions::new()
        .max_connections(max_connections)
        .min_connections(min_connections)
        .acquire_timeout(Duration::from_secs(acquire_timeout_secs));

    if let Some(pool) = pool_config {
        if let Some(idle_timeout) = pool.idle_timeout_secs {
            pool_options = pool_options.idle_timeout(Duration::from_secs(idle_timeout));
        }

        if let Some(max_lifetime) = pool.max_lifetime_secs {
            pool_options = pool_options.max_lifetime(Duration::from_secs(max_lifetime));
        }
    }

    loop {
        retry_count += 1;
        println!("Connection attempt {} of {}", retry_count, max_retries);

        match pool_options.clone().connect(database_url).await {
            Ok(pool) => {
                println!(
                    "Successfully connected to database on attempt {}",
                    retry_count
                );

                // Test the connection with a simple query
                match sqlx::query("SELECT 1").execute(&pool).await {
                    Ok(_) => {
                        println!("Database connection test successful");
                        return Ok(pool);
                    }
                    Err(e) => {
                        println!("Database connection test failed: {}", e);
                        if retry_count >= max_retries {
                            return Err(format!(
                                "Database connection test failed after {} attempts: {}",
                                max_retries, e
                            )
                            .into());
                        }
                    }
                }
            }
            Err(e) => {
                println!("Connection attempt {} failed: {}", retry_count, e);
                if retry_count >= max_retries {
                    return Err(format!(
                        "Failed to connect to database after {} attempts: {}",
                        max_retries, e
                    )
                    .into());
                }
            }
        }

        // Wait before retrying (exponential backoff)
        let delay = Duration::from_millis(100 * (1 << (retry_count - 1).min(6))); // Cap at ~6.4 seconds
        println!("Waiting {:?} before retry...", delay);
        tokio::time::sleep(delay).await;
    }
}

pub fn get_database_pool() -> Result<Arc<PgPool>, sqlx::Error> {
    DATABASE_POOL
        .get()
        .cloned()
        .ok_or(sqlx::Error::PoolTimedOut)
}

pub async fn cleanup_database() {
    println!("Cleaning up database...");

    // Close the database pool
    if let Some(pool) = DATABASE_POOL.get() {
        pool.close().await;
        println!("Database pool closed");
    }

    // Stop the PostgreSQL instance
    if let Some(postgresql_arc) = POSTGRESQL_INSTANCE.get() {
        let postgresql_arc = postgresql_arc.clone();
        tokio::task::spawn_blocking(move || {
            if let Ok(postgresql) = postgresql_arc.lock() {
                let rt = tokio::runtime::Runtime::new().unwrap();
                if let Err(e) = rt.block_on(postgresql.stop()) {
                    eprintln!("Error stopping PostgreSQL: {}", e);
                } else {
                    println!("PostgreSQL instance stopped");
                }
            }
        })
        .await
        .unwrap_or_else(|e| {
            eprintln!("Failed to stop PostgreSQL: {}", e);
        });
    }
}

fn register_cleanup_handlers() {
    // Only register once
    if CLEANUP_REGISTERED.swap(true, Ordering::SeqCst) {
        return;
    }

    // Register cleanup on panic.
    //
    // SECURITY/CORRECTNESS: 14-core F-09 (Medium). The previous
    // implementation called `tokio::runtime::Runtime::new().unwrap()`
    // from inside the panic hook, but the hook commonly fires while a
    // tokio runtime is already on the stack (any handler panic). Tokio
    // refuses to start a new runtime nested inside an existing one
    // ('Cannot start a runtime from within a runtime'), so the cleanup
    // hook double-faulted and left the embedded PostgreSQL data dir
    // unstopped. Same bug in the Drop impl below.
    //
    // The fix uses `tokio::runtime::Handle::try_current()` to detect
    // whether we're already on a tokio runtime; if so, schedule the
    // cleanup on that runtime via `block_in_place` + `block_on`; if not,
    // spin up a fresh runtime (the original behavior, now only on the
    // path where it's safe).
    let orig_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        tracing::error!("Panic detected, cleaning up database");
        run_cleanup_blocking();
        orig_hook(panic_info);
    }));
}

/// Run `cleanup_database` synchronously from a context that may or may
/// not be on a tokio runtime. Detects the runtime via Handle::try_current
/// and uses block_in_place to avoid the 'Cannot start a runtime from
/// within a runtime' double-fault when called from the panic hook
/// during an async-handler panic. 14-core F-09 (Medium).
fn run_cleanup_blocking() {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            tokio::task::block_in_place(|| handle.block_on(cleanup_database()));
        }
        Err(_) => match tokio::runtime::Runtime::new() {
            Ok(rt) => rt.block_on(cleanup_database()),
            Err(e) => {
                tracing::error!(error = %e, "Failed to create runtime for cleanup");
            }
        },
    }
}

// Drop implementation for graceful shutdown
struct DatabaseCleanup;

impl Drop for DatabaseCleanup {
    fn drop(&mut self) {
        tracing::info!("DatabaseCleanup Drop called, cleaning up database");
        run_cleanup_blocking();
    }
}

// Static instance to ensure cleanup on drop
static _CLEANUP: std::sync::LazyLock<DatabaseCleanup> =
    std::sync::LazyLock::new(|| DatabaseCleanup);

#[cfg(test)]
mod tests {
    use super::*;

    /// The Postgres binary version used for the versioned `pg_ctl` path in
    /// tests — mirrors ziee's `ZIEE_POSTGRES_VERSION` build env (the SDK
    /// workspace has no such env, so the tests use a literal).
    const TEST_PG_VERSION: &str = "18.3.0";

    fn sample_embedded_cfg() -> ziee_core::config::EmbeddedPostgreSqlConfig {
        // serde_json fills the struct without hand-writing every field name;
        // installation_dir/data_dir omitted so they deserialize to None.
        serde_json::from_str(
            r#"{
                "version":"18.3.0","port":50000,"bind_address":"127.0.0.1",
                "username":"postgres","password":"pw","database":"app",
                "timezone":"UTC","log_timezone":"UTC",
                "logging":{"collector":false,"directory":"log","filename":"pg.log","statement":"none"}
            }"#,
        )
        .unwrap()
    }

    /// P0-b: `resolve_embedded_paths` fills unset installation_dir/data_dir with
    /// app-data-dir-derived defaults (`<data dir>/postgres` + `/postgres-data`),
    /// with NO reference to a ziee-side `Config::resolve_paths`.
    #[test]
    fn resolve_embedded_paths_fills_unset_dirs() {
        let mut cfg = sample_embedded_cfg();
        assert!(cfg.installation_dir.is_none());
        assert!(cfg.data_dir.is_none());
        resolve_embedded_paths(&mut cfg);
        let base = ziee_core::app_state::get_app_data_dir();
        assert_eq!(
            cfg.installation_dir.as_deref(),
            Some(base.join("postgres").to_string_lossy().as_ref())
        );
        assert_eq!(
            cfg.data_dir.as_deref(),
            Some(base.join("postgres-data").to_string_lossy().as_ref())
        );
    }

    /// Operator/app overrides win — already-set dirs are never clobbered
    /// (equivalence-preserving for ziee, which always fills them).
    #[test]
    fn resolve_embedded_paths_preserves_set_dirs() {
        let mut cfg = sample_embedded_cfg();
        cfg.installation_dir = Some("/custom/pg".to_string());
        cfg.data_dir = Some("/custom/pgdata".to_string());
        resolve_embedded_paths(&mut cfg);
        assert_eq!(cfg.installation_dir.as_deref(), Some("/custom/pg"));
        assert_eq!(cfg.data_dir.as_deref(), Some("/custom/pgdata"));
    }

    /// No `data/postmaster.pid` under the installation dir → there's nothing
    /// running, so the stop is a clean no-op (never shells out to pg_ctl).
    #[test]
    fn stop_is_noop_when_no_postmaster_pid() {
        let dir = tempfile::tempdir().unwrap();
        // Note: no data/ dir created at all.
        let res = stop_existing_postgres_instance(&dir.path().to_path_buf(), TEST_PG_VERSION);
        assert!(res.is_ok());
    }

    /// A stale `postmaster.pid` exists but the versioned `pg_ctl` binary is
    /// absent → the function warns and returns Ok rather than erroring or
    /// exiting (exercises the cross-platform `<dir>/<version>/bin/pg_ctl[.exe]`
    /// path construction + existence check).
    #[test]
    fn stop_returns_ok_when_pg_ctl_missing() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("postmaster.pid"), "12345\n").unwrap();

        // The pg_ctl binary at <dir>/<TEST_PG_VERSION>/bin/pg_ctl does not
        // exist, so the function must take the "warn + Ok" early return.
        let res = stop_existing_postgres_instance(&dir.path().to_path_buf(), TEST_PG_VERSION);
        assert!(res.is_ok());

        // The versioned bin path we skipped is the one it would have invoked.
        let pg_ctl_exe = if cfg!(target_os = "windows") {
            "pg_ctl.exe"
        } else {
            "pg_ctl"
        };
        let expected = dir
            .path()
            .join(TEST_PG_VERSION)
            .join("bin")
            .join(pg_ctl_exe);
        assert!(!expected.exists());
    }
}
