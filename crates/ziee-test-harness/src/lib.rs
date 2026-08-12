//! `ziee-test-harness` — the app-neutral integration-test harness.
//!
//! The generic core of the former `server/tests/common/harness_inner.rs`. It
//! spawns a real app binary against a per-test isolated Postgres DB (cloned
//! from a fully-migrated template so migrations run once per process, not once
//! per test — the property that makes the suite safe WITHOUT `--test-threads=1`),
//! allocates a free TCP port, writes a temp config file, health-polls the
//! server up, and reaps everything on `Drop` (kill child, remove config, async
//! `DROP DATABASE`, drop the isolated data-dir + any app keep-alive tempdirs).
//!
//! ## The seam
//!
//! Every app coupling is threaded through the [`HarnessApp`] trait an app
//! implements once. The harness names ONLY that seam — never `ziee`. In
//! particular the harness NEVER calls `env!("CARGO_MANIFEST_DIR")`: inside this
//! compiled SDK crate that would resolve to the SDK crate's own dir and break
//! the repo-root `.ziee-cache` walk, the per-worktree DB keying, the migration
//! roots, and the binary walk. Instead every one of those uses a runtime
//! `manifest_dir: PathBuf` the CONSUMER passes from its OWN
//! `env!("CARGO_MANIFEST_DIR")` at the shim site.

use std::any::Any;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// Re-export the per-worktree DB keying so a consumer keeps one import; shared
// with build.rs (via ziee-build-support) so the suffix derivation is identical
// on both sides.
pub use ziee_build_support::worktree_db;

// App-neutral auth/sync test fixtures. Two independently-selectable groups —
// `sync-probe` (the SSE reader) and `auth-mocks` (oauth/ldap/apple) — so an app
// can take the cheap one without the docker/HTTP-mock deps of the other.
// `fixtures` = both, which is what it always meant.
#[cfg(any(feature = "sync-probe", feature = "auth-mocks"))]
pub mod fixtures;

/// Seam for [`fixtures::sync_probe::SyncProbe::open`]: the consuming app's thin
/// `TestServer` shim implements this so the probe can build the
/// `/sync/subscribe` URL without the fixture naming any app-side server type.
/// Dep-free and always compiled (the impl lives on the app side, so it must not
/// hide behind the `fixtures` feature).
pub trait ApiUrlTarget {
    /// Absolute URL for an API path (e.g. `"/sync/subscribe"` → base + `/api…`).
    fn api_url(&self, path: &str) -> String;
}

/// Which app crate this test binary is compiled into. Replaces the former
/// compile-time `is_desktop()` (a `CARGO_PKG_NAME` check) with a RUNTIME value
/// the consumer's per-crate shim seeds (the shim is still `#[path]`-compiled
/// per crate, so it can read its own `CARGO_PKG_NAME` correctly). Drives the
/// template DB name + the migration set — the two things that legitimately
/// differ between a server-only and a desktop test binary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Server,
    Desktop,
}

/// Parsed Postgres connection parts (from `DATABASE_URL`), minus the per-test
/// database name (which the engine generates). Handed to the app so its
/// `plan_spawn` can render the external-DB block of its config.
#[derive(Debug, Clone)]
pub struct DbConn {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

/// Everything the engine has computed for this spawn that the app needs to
/// render its config. The app OWNS the config content (issuer/audience literals,
/// feature sections); the harness owns these generic facts.
pub struct SpawnFacts<'a> {
    pub db: &'a DbConn,
    /// The generated per-test database name (already cloned from the template).
    pub database_name: &'a str,
    /// The free port the server must bind.
    pub server_port: u16,
    /// The per-test isolated `data_dir` (mutable state fresh per test; binary
    /// caches symlinked-in shared).
    pub data_dir: &'a Path,
    /// The unique test id (usable for per-test override dirs, filenames, etc.).
    pub test_id: &'a str,
}

/// The app's per-spawn plan: the rendered config plus how to launch + what to
/// keep alive. Bundling these into one seam call (rather than separate
/// `render_config`/`extra_argv`/`extra_env` methods) lets the app create
/// per-test tempdirs ONCE and thread their PATH into `config_yaml`/`extra_env`
/// while parking their HANDLE in `keep_alive` for the engine to drop at test
/// end — the config↔lifetime coupling that separate methods can't express.
pub struct SpawnPlan {
    /// The full config YAML written to a temp file and passed via
    /// `--config-file`.
    pub config_yaml: String,
    /// The binary stem to spawn (e.g. `"ziee"` / `"ziee-desktop"`). The engine
    /// appends `.exe` on Windows and walks up from `manifest_dir` to find it
    /// under `target/debug/`.
    pub binary_name: String,
    /// Extra CLI args (e.g. `["--headless"]` for a desktop-binary spawn).
    pub extra_argv: Vec<String>,
    /// Extra process env vars (e.g. a per-test global-state override dir).
    pub extra_env: Vec<(String, String)>,
    /// Handles (typically `tempfile::TempDir`) whose lifetime must match the
    /// spawned server's. The engine parks them on the returned handle and drops
    /// them (deleting their trees) when the test ends.
    pub keep_alive: Vec<Box<dyn Any + Send + Sync>>,
}

/// The app-implemented integration-test boundary. One impl per app; the harness
/// is generic over it. Every method is synchronous (all app couplings today —
/// storage-key init, Windows sandbox-helper install, config string formatting,
/// tempdir creation — are synchronous, so the seam needs no async-trait).
pub trait HarnessApp: Send + Sync + 'static {
    /// The app's option struct (today's `TestServerOptions`). The harness owns
    /// only the generic [`SpawnFacts`]; every app-specific knob lives here.
    type Options: Default + Clone + Send;

    /// Base name of the fully-migrated TEMPLATE database for this variant. The
    /// harness appends the per-worktree suffix so concurrent worktrees never
    /// clobber each other's template.
    fn template_db_base(&self, variant: Variant) -> String;

    /// Ordered migration directories to apply when building the template,
    /// resolved against the consumer's `manifest_dir`.
    fn migration_dirs(&self, variant: Variant, manifest_dir: &Path) -> Vec<PathBuf>;

    /// Process-global side effects to run before the first spawn (e.g.
    /// initialise an at-rest secret key in the test process, install a Windows
    /// helper service). Default: no-op.
    fn before_spawn(&self, _opts: &Self::Options) {}

    /// Render the config + decide the launch plan for this spawn.
    fn plan_spawn(&self, opts: &Self::Options, facts: &SpawnFacts) -> SpawnPlan;

    /// Health endpoint the readiness loop polls (relative to the base URL).
    fn health_path(&self) -> String {
        "/api/health".to_string()
    }
}

/// Get the admin database URL from the environment or the shared-cluster default.
fn admin_database_url() -> String {
    env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

/// Stable per-worktree suffix for this test binary's template DB, derived from
/// the CONSUMER's worktree root. Empty when `DATABASE_URL` is a deliberate
/// override or auto-isolate is opted out — preserving the historical
/// single-worktree template names.
fn worktree_suffix(manifest_dir: &Path) -> String {
    let explicit = env::var("DATABASE_URL").ok();
    if worktree_db::should_auto_isolate(&explicit) {
        format!(
            "_{}",
            worktree_db::worktree_key(&manifest_dir.to_string_lossy())
        )
    } else {
        String::new()
    }
}

/// Repo-relative shared cache dir for tests. Injected as `app.data_dir` (via the
/// isolated dir below) so binary extractions (pandoc/pdfium/uv/bun + the sandbox
/// runtime) happen ONCE across `cargo test` invocations, and tests never fall
/// back to the dev's real `~/.ziee/`. Lives under `.ziee-cache/` (gitignored).
///
/// `manifest_dir` is the CONSUMER's `CARGO_MANIFEST_DIR`; the repo root is two
/// levels up (`src-app/server` → `src-app` → repo) — preserved verbatim from
/// the pre-move harness, including the desktop-crate resolution.
pub fn shared_test_app_data_dir(manifest_dir: &Path) -> PathBuf {
    let path = manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(|repo| repo.join(".ziee-cache").join("test-app-data"))
        .expect("repo root walk");
    fs::create_dir_all(&path).expect("create shared test app_data_dir");
    path
}

/// Per-test isolated `app.data_dir` that keeps the EXPENSIVE binary caches
/// shared. Each test gets a fresh TempDir for its MUTABLE state — which is what
/// makes the suite safe to run WITHOUT `--test-threads=1` — while the read-only
/// extracted caches (`bin/` = pandoc/pdfium/uv/bun, `lib/`, `llm-engines/`,
/// `lit-cache/`) are SYMLINKED in from the shared `.ziee-cache` dir so the
/// hundreds-of-MB extraction still happens once per `cargo test` run. Non-unix
/// falls back to the shared dir (the CI parallel target is linux).
pub fn make_isolated_data_dir(manifest_dir: &Path) -> tempfile::TempDir {
    let shared = shared_test_app_data_dir(manifest_dir);
    let td = tempfile::Builder::new()
        .prefix("ziee-test-data-")
        .tempdir()
        .expect("create per-test data_dir TempDir");
    // Symlink the read-only / content-addressed caches so they stay shared.
    // `lib` is load-bearing for the macOS sandbox: the embedded sandbox-runtime
    // bundle extracts its launcher to `bin/` and its dylibs (libkrun, …) to
    // `lib/`, and the launcher's rpath is `@executable_path/../lib`. Keeping
    // `lib` shared alongside `bin` co-locates them exactly as in a production
    // single-app_data layout.
    for sub in ["bin", "lib", "llm-engines", "lit-cache"] {
        let target = shared.join(sub);
        fs::create_dir_all(&target).ok();
        #[cfg(unix)]
        {
            let _ = std::os::unix::fs::symlink(&target, td.path().join(sub));
        }
    }
    td
}

// Per-test DBs are cloned from a fully-migrated TEMPLATE via
// `CREATE DATABASE ... TEMPLATE`, so migrations run exactly ONCE per test
// process instead of once per test — eliminating the per-test migration races
// that broke parallel runs (a half-applied schema → "relation does not exist")
// and making DB setup dramatically faster (a byte-copy vs replaying every
// migration per test).

static TEST_TEMPLATE: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// The fully-migrated TEMPLATE database name = the app's per-variant base +
/// the per-worktree suffix.
fn test_template_db<A: HarnessApp>(app: &A, variant: Variant, manifest_dir: &Path) -> String {
    format!(
        "{}{}",
        app.template_db_base(variant),
        worktree_suffix(manifest_dir)
    )
}

/// Build the migrated template DB exactly once per process (the OnceCell makes
/// every concurrent test await the single build before any of them clone). The
/// template must have NO active connections when a clone runs, so we close our
/// pools and terminate any stragglers before returning.
async fn ensure_test_template<A: HarnessApp>(
    admin_url: &str,
    app: &A,
    variant: Variant,
    manifest_dir: &Path,
) {
    TEST_TEMPLATE
        .get_or_init(|| async {
            let admin = PgPoolOptions::new()
                .max_connections(1)
                .connect(admin_url)
                .await
                .expect("connect postgres to build test template");
            let template_db = test_template_db(app, variant, manifest_dir);
            let term = format!(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{template_db}' AND pid <> pg_backend_pid()"
            );
            let _ = sqlx::query(&term).execute(&admin).await;
            // Rebuild fresh each process so migration changes are picked up.
            let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {template_db}"))
                .execute(&admin)
                .await;
            sqlx::query(&format!("CREATE DATABASE {template_db}"))
                .execute(&admin)
                .await
                .expect("create test template database");
            admin.close().await;

            // Migrate the template at RUNTIME from the on-disk migration dirs.
            // We deliberately do NOT use the compile-time crate-relative
            // `sqlx::migrate!("./migrations")` macro: compiled into a desktop
            // test binary it would resolve to the desktop crate's own dir and
            // miss the server's migrations. The runtime Migrator lets a desktop
            // build apply server-then-desktop from the dirs the app supplies.
            let mut tmpl = url::Url::parse(admin_url).expect("admin url");
            tmpl.set_path(&template_db);
            let tmpl_pool = PgPoolOptions::new()
                .max_connections(1)
                .connect(tmpl.as_str())
                .await
                .expect("connect template database");
            for dir in app.migration_dirs(variant, manifest_dir) {
                let mut migrator = sqlx::migrate::Migrator::new(dir.clone())
                    .await
                    .unwrap_or_else(|e| panic!("create migrator for {}: {e}", dir.display()));
                // A later migration set (e.g. desktop) carries version numbers
                // above an earlier one; ignore-missing lets each migrator run
                // against a DB that already has the other set applied.
                migrator.set_ignore_missing(true);
                migrator
                    .run(&tmpl_pool)
                    .await
                    .unwrap_or_else(|e| panic!("migrate test template from {}: {e}", dir.display()));
            }
            tmpl_pool.close().await;

            // Drop any lingering backend on the template so clones can copy it.
            let admin2 = PgPoolOptions::new()
                .max_connections(1)
                .connect(admin_url)
                .await
                .expect("connect postgres to quiesce template");
            let _ = sqlx::query(&term).execute(&admin2).await;
            admin2.close().await;
        })
        .await;
}

/// A running test server: a spawned app process bound to a fresh per-test DB.
/// Owns the child process, the temp config, the isolated data-dir, and any app
/// keep-alive handles; `Drop` reaps them all + drops the per-test database.
///
/// The public string fields (`base_url`/`database_name`/`database_url`) are what
/// tests read; the consumer's thin `TestServer` shim wraps this handle and
/// re-exposes them + `api_url`/`data_dir` with identical names/signatures.
pub struct SpawnedServer {
    process: Child,
    pub base_url: String,
    pub database_name: String,
    pub database_url: String,
    temp_config_path: PathBuf,
    admin_db_url: String,
    /// Per-test isolated data_dir (mutable state); dropped at test end.
    _data_tempdir: tempfile::TempDir,
    /// App-supplied handles (workspace/hub/sandbox-cache tempdirs) held for the
    /// server's lifetime; dropped at test end.
    _keep_alive: Vec<Box<dyn Any + Send + Sync>>,
}

impl SpawnedServer {
    /// The spawned server process's per-test `app.data_dir`.
    pub fn data_dir(&self) -> &Path {
        self._data_tempdir.path()
    }
}

impl Drop for SpawnedServer {
    fn drop(&mut self) {
        // Kill the server process.
        let _ = self.process.kill();
        let _ = self.process.wait();

        // Delete the temporary config file.
        let _ = fs::remove_file(&self.temp_config_path);

        // Cleanup database.
        let database_name = self.database_name.clone();
        let db_url = self.admin_db_url.clone();

        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let _ = handle.spawn(async move {
                if let Ok(pool) = PgPoolOptions::new()
                    .max_connections(1)
                    .connect(&db_url)
                    .await
                {
                    // Terminate existing connections.
                    let _ = sqlx::query(&format!(
                        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '{}' AND pid <> pg_backend_pid()",
                        database_name
                    ))
                    .execute(&pool)
                    .await;

                    // Drop the database.
                    let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {}", database_name))
                        .execute(&pool)
                        .await;

                    pool.close().await;
                }
            });
        }
    }
}

/// The harness: an installed [`HarnessApp`] impl + the consumer's `manifest_dir`
/// + the compile-context [`Variant`]. Construct once per test binary (a
/// `OnceLock` in the consumer's shim) and call [`TestHarness::start`] per test.
pub struct TestHarness<A: HarnessApp> {
    app: A,
    manifest_dir: PathBuf,
    variant: Variant,
}

impl<A: HarnessApp> TestHarness<A> {
    /// `manifest_dir` MUST be the CONSUMER's `env!("CARGO_MANIFEST_DIR")`,
    /// evaluated at the shim site — NOT this crate's. It anchors the repo-root
    /// cache walk, the per-worktree DB key, the migration roots, and the binary
    /// walk; the wrong value silently splits caches / fails the template build /
    /// can't find the binary.
    pub fn new(app: A, manifest_dir: PathBuf, variant: Variant) -> Self {
        Self {
            app,
            manifest_dir,
            variant,
        }
    }

    /// Spawn a fresh test server with the given app options.
    pub async fn start(&self, opts: A::Options) -> SpawnedServer {
        // Process-global pre-spawn side effects (storage-key init, Windows
        // sandbox-helper install). Idempotent by the app's contract.
        self.app.before_spawn(&opts);

        // Unique identifiers.
        let test_id = Uuid::new_v4().to_string();
        let database_name = format!("test_db_{}", test_id.replace('-', "_"));

        // OS-aware free-port reservation (avoids the "Address already in use"
        // boot-timeout cluster a random pick caused).
        let server_port =
            portpicker::pick_unused_port().expect("No free TCP port available for TestServer");

        // Parse DATABASE_URL into connection parts.
        let db_url = admin_database_url();
        let url = url::Url::parse(&db_url).expect("Invalid DATABASE_URL");
        let db = DbConn {
            host: url.host_str().unwrap_or("127.0.0.1").to_string(),
            port: url.port().unwrap_or(54321),
            username: url.username().to_string(),
            password: url.password().unwrap_or("").to_string(),
        };

        // Per-test isolated data_dir (mutable state fresh per test; binary
        // caches symlinked-in shared). Held on the handle so its tree is reaped.
        let data_tempdir = make_isolated_data_dir(&self.manifest_dir);
        let data_dir_path = data_tempdir.path().to_path_buf();

        // Let the app render the config + decide the launch plan.
        let plan = self.app.plan_spawn(
            &opts,
            &SpawnFacts {
                db: &db,
                database_name: &database_name,
                server_port,
                data_dir: &data_dir_path,
                test_id: &test_id,
            },
        );

        // Write the temp config (cross-platform temp dir).
        let temp_config_path = std::env::temp_dir().join(format!("testharness-{test_id}.yaml"));
        fs::write(&temp_config_path, &plan.config_yaml)
            .expect("Failed to write temporary config");

        // Ensure the fully-migrated template exists (built once per process),
        // then clone the per-test DB from it — no migrations run per test.
        ensure_test_template(&db_url, &self.app, self.variant, &self.manifest_dir).await;

        let pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&db_url)
            .await
            .expect("Failed to connect to PostgreSQL - ensure docker compose is running");

        sqlx::query(&format!(
            "CREATE DATABASE {} TEMPLATE {}",
            database_name,
            test_template_db(&self.app, self.variant, &self.manifest_dir)
        ))
        .execute(&pool)
        .await
        .expect("Failed to create test database from template");

        pool.close().await;

        // Resolve the binary path. Windows appends `.exe`. The workspace layout
        // puts `target/` at `src-app/target`, so walk up from `manifest_dir`:
        //   - server crate: manifest=src-app/server, parent=src-app ✓
        //   - desktop crate: manifest=src-app/desktop/tauri,
        //     parent=src-app/desktop (no target), grandparent=src-app ✓
        let exe_name = if cfg!(windows) {
            format!("{}.exe", plan.binary_name)
        } else {
            plan.binary_name.clone()
        };
        let binary_path = {
            let candidates = [
                self.manifest_dir
                    .parent()
                    .map(|p| p.join("target/debug").join(&exe_name)),
                self.manifest_dir
                    .parent()
                    .and_then(|p| p.parent())
                    .map(|p| p.join("target/debug").join(&exe_name)),
                Some(self.manifest_dir.join("target/debug").join(&exe_name)),
            ];
            candidates
                .into_iter()
                .flatten()
                .find(|p| p.exists())
                .unwrap_or_else(|| self.manifest_dir.join("target/debug").join(&exe_name))
        };

        // Start the server process with the temp config + app argv/env.
        let mut cmd = Command::new(&binary_path);
        cmd.arg("--config-file").arg(&temp_config_path);
        for arg in &plan.extra_argv {
            cmd.arg(arg);
        }
        for (k, v) in &plan.extra_env {
            cmd.env(k, v);
        }
        let child = cmd.spawn().expect("Failed to start test server");

        let base_url = format!("http://127.0.0.1:{}", server_port);
        let test_database_url = format!(
            "postgresql://{}:{}@{}:{}/{}",
            db.username, db.password, db.host, db.port, database_name
        );

        // Wait for the server to be ready (150 × 200ms = 30s ceiling; boot runs
        // the security middleware + module registration + external Postgres
        // connect, which can exceed a few seconds on a busy box).
        let client = reqwest::Client::new();
        let health_url = format!("{}{}", base_url, self.app.health_path());
        let mut ready = false;
        for _ in 0..150 {
            if let Ok(response) = client.get(&health_url).send().await {
                if response.status().is_success() {
                    ready = true;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !ready {
            panic!("TestServer at {} did not become healthy within 30s", base_url);
        }

        SpawnedServer {
            process: child,
            base_url,
            database_name,
            database_url: test_database_url,
            temp_config_path,
            admin_db_url: db_url,
            _data_tempdir: data_tempdir,
            _keep_alive: plan.keep_alive,
        }
    }
}
