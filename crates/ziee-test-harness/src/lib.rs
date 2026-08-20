//! `ziee-test-harness` — the app-neutral integration-test harness.
//!
//! The generic core of the former `server/tests/common/harness_inner.rs`. It
//! spawns a real app binary against a per-test isolated Postgres DB (cloned
//! from a fully-migrated template so migrations run once per process, not once
//! per test — the property that makes the suite safe WITHOUT `--test-threads=1`),
//! allocates a free TCP port, writes a temp config file, health-polls the
//! server up, and reaps everything on `Drop` — kill child, remove config,
//! SYNCHRONOUSLY `DROP DATABASE` (see [`SpawnedServer::drop`] for why the word
//! matters), drop the isolated data-dir + any app keep-alive tempdirs. The exit
//! paths a destructor cannot reach are covered by [`sweep_stale_test_dbs`] at the
//! start of the next test process.
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

/// The per-test `app.data_dir`: owned, unmounted before removal, and swept at the
/// start of the next process. See the module docs for why a bare `TempDir` leaked
/// 176 GB and 703 mount-table entries on the happy path.
pub mod data_dir;

pub use data_dir::{
    sweep_stale_test_data_dirs, DataDirSweepReport, PerTestDataDir, TEST_DATA_DIR_PREFIX,
};

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
///
/// Returns a [`PerTestDataDir`], NOT a `tempfile::TempDir`: the tree routinely
/// contains a squashfuse mount, and `TempDir`'s destructor is a `remove_dir_all`
/// whose error is discarded — which over this tree means it deletes the symlinks
/// below, fails with `ENOSYS` at the mount point, and silently leaves ~300 MB and a
/// `/proc/mounts` entry behind on every green run. See [`data_dir`].
pub fn make_isolated_data_dir(manifest_dir: &Path) -> PerTestDataDir {
    let shared = shared_test_app_data_dir(manifest_dir);
    let td = PerTestDataDir::new().expect("create per-test data_dir");
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

/// Prefix of every per-test database this engine creates. The ONE literal — the
/// creator ([`TestHarness::start`]), the reaper ([`SpawnedServer::drop`]) and the
/// sweep ([`sweep_stale_test_dbs`]) all key on it, so they cannot drift apart into
/// "creates one shape, collects another".
pub const TEST_DB_PREFIX: &str = "test_db_";

/// Default age floor for [`sweep_stale_test_dbs`], in seconds.
///
/// A per-test database is created immediately before its server spawns and reaped
/// when the test's handle drops, so its whole life is one test's wall time — bounded
/// above by the 30s readiness ceiling plus the body, i.e. minutes at the very worst.
/// Two hours is two orders of magnitude of margin over that, which is the point: the
/// floor is not a tuning knob, it is the distance between "no test is this slow" and
/// "a concurrent run could still be using this".
const DEFAULT_SWEEP_MIN_AGE_SECS: i64 = 7200;

/// Upper bound on one sweep, so a badly-leaked cluster costs a bounded startup
/// rather than minutes of `DROP DATABASE`. The sweep is self-healing across runs, so
/// a capped pass still converges.
const SWEEP_MAX_PER_PASS: i64 = 2000;

/// Reclaim per-test databases that no `Drop` will ever reap.
///
/// `Drop` covers the normal return, the early `?` return and the panicking body. It
/// covers NONE of `panic = "abort"`, a SIGKILL, an OOM kill, or a test-binary
/// timeout — and those are precisely the runs that leak in bulk. This sweep is the
/// self-healing half: it runs once per test process, before the first clone, and
/// needs no cooperation from the run that leaked.
///
/// ## What it will and will not take
///
/// A database is collected only when ALL of:
///
///   1. its name matches the anchored `test_db_` prefix — never the shared
///      `<app>_test_template_<key>` or `<app>_build_<key>` databases, which a whole
///      worktree's concurrent run depends on and which carry no such prefix;
///   2. it has ZERO sessions in `pg_stat_activity`;
///   3. the creation time of its `PG_VERSION` file is older than `min_age_secs`.
///
/// Neither (2) nor (3) is sufficient alone, which is why both are required. (2)
/// alone is unsafe because a LIVE test's database genuinely can show zero backends:
/// the harness renders `idle_timeout_secs: 10` into the server's pool config, so a
/// test that pauses ten seconds between requests has no connections while very much
/// still running. (3) alone is unsafe because it is a wall-clock guess about a
/// stranger's process. Together they describe a database that both looks abandoned
/// and has had far longer than any test takes to prove otherwise.
///
/// The drop itself is a plain `DROP DATABASE` — deliberately **not** `WITH (FORCE)`.
/// FORCE would terminate the sessions of whatever is connected, converting the
/// residual race in (2)→(3) from "the drop errors and we log it" into "a concurrent
/// run loses its database mid-test". Erroring is the correct outcome here, so the
/// non-forcing form is load-bearing, not an oversight.
///
/// Returns a [`SweepReport`]. It carries `considered` — every name the query
/// admitted — and not merely `dropped`, because those two answer different
/// questions and only the first can guard the in-use check. A sweep that has lost
/// its in-use check still fails to drop a live database (Postgres refuses a
/// non-forcing `DROP` while a session is attached), so a control that only inspects
/// `dropped` goes green on exactly the mutation it exists to catch. `considered` is
/// the set the sweep BELIEVED was abandoned, which is the thing under test.
pub async fn sweep_stale_test_dbs(admin_url: &str, min_age_secs: i64) -> SweepReport {
    let mut report = SweepReport::default();

    let pool = match PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(admin_url)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            eprintln!("test harness: stale-db sweep could not connect: {e}");
            return report;
        }
    };

    // `pg_stat_file` needs superuser / `pg_read_server_files`. The harness connects
    // as the cluster's admin role, but a deployment that does not may lose the age
    // column — in which case the sweep does NOTHING rather than falling back to a
    // less safe discriminator. Losing the sweep re-opens a leak; losing the age
    // check would let it eat a live run.
    let sql = format!(
        "SELECT d.datname \
           FROM pg_database d \
          WHERE d.datname LIKE '{TEST_DB_PREFIX}%' \
            AND NOT d.datistemplate \
            AND NOT EXISTS ( \
                  SELECT 1 FROM pg_stat_activity a WHERE a.datname = d.datname) \
            AND (pg_stat_file('base/' || d.oid || '/PG_VERSION')).modification \
                  < now() - make_interval(secs => $1) \
          LIMIT {SWEEP_MAX_PER_PASS}"
    );

    let rows: Vec<(String,)> = match sqlx::query_as(&sql)
        .bind(min_age_secs as f64)
        .fetch_all(&pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            eprintln!("test harness: stale-db sweep could not enumerate candidates: {e}");
            pool.close().await;
            return report;
        }
    };

    for (name,) in rows {
        // Never interpolate a name that is not the shape we create. Belt to the
        // LIKE's braces: the identifier goes into DDL, where a bind parameter is
        // not available.
        if !is_engine_test_db_name(&name) {
            continue;
        }
        report.considered.push(name.clone());
        match sqlx::query(&format!("DROP DATABASE IF EXISTS {name}"))
            .execute(&pool)
            .await
        {
            Ok(_) => report.dropped.push(name),
            // Expected and fine: something connected between the check and the
            // drop, so Postgres refused. That refusal is the residual-race safety
            // net, NOT the in-use check — which is why it is recorded separately.
            Err(e) => {
                eprintln!("test harness: stale-db sweep left {name} in place: {e}");
                report.refused.push(name);
            }
        }
    }

    pool.close().await;
    if !report.dropped.is_empty() {
        eprintln!(
            "test harness: stale-db sweep reclaimed {} orphaned {TEST_DB_PREFIX}* database(s)",
            report.dropped.len()
        );
    }
    report
}

/// What one [`sweep_stale_test_dbs`] pass did.
#[derive(Debug, Default, Clone)]
pub struct SweepReport {
    /// Every database the sweep judged abandoned and attempted to drop. This is
    /// the set the safety controls assert against: a database still in use must
    /// never reach it, whether or not the drop would then have failed.
    pub considered: Vec<String>,
    /// Successfully dropped.
    pub dropped: Vec<String>,
    /// Considered, but Postgres refused (something connected in between).
    pub refused: Vec<String>,
}

/// Exactly the shape [`TestHarness::start`] generates: the prefix followed by a
/// UUID with its hyphens replaced by underscores. Anything else is somebody
/// else's database and is never interpolated into DDL.
fn is_engine_test_db_name(name: &str) -> bool {
    match name.strip_prefix(TEST_DB_PREFIX) {
        Some(rest) => !rest.is_empty() && rest.chars().all(|c| c.is_ascii_hexdigit() || c == '_'),
        None => false,
    }
}

static STARTUP_SWEEP_RAN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the once-per-process startup sweep hook has run in THIS process.
///
/// Exists so a test can assert the sweep is INSTALLED, not merely callable. Every
/// other sweep test invokes [`sweep_stale_test_dbs`] directly and would stay green
/// if the call inside the harness's own startup were deleted — which is the half of
/// this fix that covers SIGKILL, OOM, `panic = "abort"` and binary timeouts.
pub fn startup_sweep_ran() -> bool {
    STARTUP_SWEEP_RAN.load(std::sync::atomic::Ordering::SeqCst)
}

/// Run [`sweep_stale_test_dbs`] once per test process with the configured floor.
///
/// `ZIEE_TEST_DB_SWEEP=0` disables it; `ZIEE_TEST_DB_SWEEP_MIN_AGE_SECS` overrides
/// the floor (the harness's own control tests call the function directly rather than
/// going through this, so the knobs are not load-bearing for correctness).
async fn sweep_stale_test_dbs_once(admin_url: &str) {
    // Recorded BEFORE the opt-out, because what this flag attests is that the
    // startup hook RAN — not what it decided. A guard on `sweep_stale_test_dbs`
    // is not a guard on anything calling it: deleting the call site leaves every
    // direct-call test green while the self-healing half of the fix is simply
    // gone. See [`startup_sweep_ran`].
    STARTUP_SWEEP_RAN.store(true, std::sync::atomic::Ordering::SeqCst);
    if env::var("ZIEE_TEST_DB_SWEEP").as_deref() == Ok("0") {
        return;
    }
    let min_age = env::var("ZIEE_TEST_DB_SWEEP_MIN_AGE_SECS")
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(DEFAULT_SWEEP_MIN_AGE_SECS);
    sweep_stale_test_dbs(admin_url, min_age).await;
}

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
            // Reclaim the orphans no `Drop` could have reaped (abort/SIGKILL/OOM/
            // timeout), BEFORE this process starts cloning its own. Self-healing:
            // it needs nothing from the run that leaked.
            sweep_stale_test_dbs_once(admin_url).await;
            // The same half of the same problem, for the per-test DATA DIR — whose
            // orphans cost ~300 MB and a `/proc/mounts` entry each, not a catalog
            // row. Runs here rather than in `start` so it is once per PROCESS.
            data_dir::sweep_stale_test_data_dirs_once();

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
    /// Declared AFTER `process`/`temp_config_path` so it drops after them: the
    /// child's Postgres sessions are gone before the reap asks for the database.
    _db: PerTestDb,
    /// Per-test isolated data_dir (mutable state), reclaimed at test end.
    ///
    /// Ordering is load-bearing and is provided by [`SpawnedServer::drop`]'s BODY,
    /// which kills and `wait`s the child before any field is dropped. The server
    /// must be dead before this field's destructor unmounts its squashfuse mounts —
    /// a live server holding the mount makes the unmount fail and the tree
    /// un-removable, which is the state this whole type exists to prevent.
    _data_tempdir: PerTestDataDir,
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

/// Drop the per-test database, synchronously. Errors are RETURNED, never
/// discarded: a `DROP DATABASE` that fails and a `DROP DATABASE` that never ran
/// leave the cluster in the identical state, which is exactly how this leak stayed
/// invisible for the whole life of the harness.
async fn reap_test_database(admin_url: &str, database_name: &str) -> Result<(), sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .connect(admin_url)
        .await?;

    // The server child was SIGKILLed a moment ago; Postgres may not have reaped its
    // backends yet, and `DROP DATABASE` refuses while any session is attached. This
    // terminates OUR OWN test's sessions only — the statement is scoped to this
    // per-test database, never the template and never anyone else's.
    let terminate = sqlx::query(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity \
          WHERE datname = $1 AND pid <> pg_backend_pid()",
    )
    .bind(database_name)
    .execute(&pool)
    .await;

    let dropped = sqlx::query(&format!("DROP DATABASE IF EXISTS {database_name}"))
        .execute(&pool)
        .await;

    pool.close().await;
    terminate?;
    dropped?;
    Ok(())
}

/// Sole owner of one per-test database, from the moment `CREATE DATABASE` returns
/// until it is reaped.
///
/// It exists because the reap used to hang off [`SpawnedServer`], which is not
/// constructed until the server is HEALTHY — leaving a window between the create and
/// that construction in which two `panic!`/`expect` sites unwind past an unowned
/// database: `Command::spawn` (a missing or unbuilt binary) and the 30s readiness
/// ceiling. The second is not hypothetical — it fired during this change's own
/// full-suite validation run on the shared box. Ownership now starts where the
/// resource does, so the window has no width rather than a shorter one.
struct PerTestDb {
    name: String,
    admin_url: String,
}

impl Drop for PerTestDb {
    /// Reap SYNCHRONOUSLY, on a thread this destructor owns and joins.
    ///
    /// ## What was wrong, and why the shape (not the call) is the fix
    ///
    /// This used to be `tokio::runtime::Handle::current().spawn(async { … })` on
    /// [`SpawnedServer`]. A test's handle drops at the end of its body, so that task
    /// was scheduled onto the very runtime `#[tokio::test]`'s `block_on` is about to
    /// return from; dropping a current-thread runtime does not poll its pending
    /// tasks, so the task was cancelled at its first `.await` — a Postgres connect —
    /// and the database was never touched. Measured: one ordinary PASSING test
    /// leaked exactly one database, i.e. a 100% leak rate. That is where 8,753
    /// orphaned `test_db_*` databases on the shared build cluster came from; at
    /// ~46,000, Postgres startup took minutes just scanning them.
    ///
    /// The fix is not "remember to clean up" — a call at the end of a test body is
    /// skipped by every early `?`, every early return and every failed assertion.
    /// The reap is bound to a VALUE's lifetime, so the language runs it on all of
    /// those paths, and it runs on a dedicated OS thread with its own runtime which
    /// is then JOINED. Owning the thread is what makes it uncancellable: nothing
    /// about the caller's runtime — its flavour, its shutdown, or its imminent
    /// unwind — can discard the work, and drop cannot return until the cluster has
    /// answered.
    ///
    /// It still cannot cover `panic = "abort"`, a SIGKILL, an OOM kill or a
    /// test-binary timeout. Nothing in this process can. Those are
    /// [`sweep_stale_test_dbs`]'s job, at the START of the next test process, which
    /// needs no cooperation from the run that died.
    fn drop(&mut self) {
        // DECISION — a failing test drops its database rather than keeping it. The
        // name is a v4 UUID printed nowhere, the server process is already dead and
        // the data dir is a `TempDir` that goes with it, so a retained database is
        // not a debugging artifact; it is 8,753 of them. Retention is available
        // DELIBERATELY and per-run, and prints the URL so the kept database can
        // actually be reached — and because the startup sweep reclaims it later,
        // opting in cannot re-open the leak.
        if env::var("ZIEE_TEST_KEEP_DB").as_deref() == Ok("1") {
            eprintln!(
                "test harness: ZIEE_TEST_KEEP_DB=1 — keeping {}{}",
                self.name,
                if std::thread::panicking() {
                    " [test was failing]"
                } else {
                    ""
                }
            );
            return;
        }

        let name = self.name.clone();
        let admin_url = self.admin_url.clone();

        // A dedicated thread with its OWN runtime: `block_on` inside a runtime
        // thread would panic, and a task on the CALLER's runtime is what leaked.
        let worker = std::thread::Builder::new()
            .name("harness-db-reap".to_string())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("test harness: LEAKED {name} — no runtime to reap it: {e}");
                        return;
                    }
                };
                rt.block_on(async {
                    // Bounded, so a wedged cluster costs one test a timeout instead
                    // of hanging the whole binary inside a destructor.
                    match tokio::time::timeout(
                        Duration::from_secs(30),
                        reap_test_database(&admin_url, &name),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            eprintln!("test harness: LEAKED {name} — DROP DATABASE failed: {e}")
                        }
                        Err(_) => {
                            eprintln!("test harness: LEAKED {name} — DROP DATABASE timed out")
                        }
                    }
                });
            });

        match worker {
            // Joining is the whole point: without it this is the old detached
            // spawn wearing a thread instead of a task.
            Ok(handle) => {
                if handle.join().is_err() {
                    eprintln!("test harness: LEAKED {} — reap thread panicked", self.name);
                }
            }
            Err(e) => eprintln!(
                "test harness: LEAKED {} — could not spawn a reap thread: {e}",
                self.name
            ),
        }
    }
}

impl Drop for SpawnedServer {
    /// Kill the child and remove the temp config. The per-test DATABASE is NOT
    /// handled here — it belongs to the [`PerTestDb`] field, whose drop runs
    /// immediately after this body and which has owned it since `CREATE DATABASE`
    /// returned, i.e. since well before this struct existed.
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
        let _ = fs::remove_file(&self.temp_config_path);
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
        fs::write(&temp_config_path, &plan.config_yaml).expect("Failed to write temporary config");

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

        // The database now EXISTS. Take ownership before anything below can panic —
        // writing the config, a missing binary, the readiness ceiling — so an unwind
        // from any of them reaps it. This assignment is the whole no-leak property;
        // everything after it is inside the guard's scope.
        let db_guard = PerTestDb {
            name: database_name.clone(),
            admin_url: db_url.clone(),
        };

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
            panic!(
                "TestServer at {} did not become healthy within 30s",
                base_url
            );
        }

        SpawnedServer {
            process: child,
            base_url,
            database_name,
            database_url: test_database_url,
            temp_config_path,
            _db: db_guard,
            _data_tempdir: data_tempdir,
            _keep_alive: plan.keep_alive,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sweep interpolates a database name straight into DDL, so the predicate
    /// that admits a name is the only thing standing between a `pg_database` row
    /// and `DROP DATABASE <that row>`. These are the cases that must NOT be admitted.
    #[test]
    fn only_the_engines_own_test_db_names_are_admitted() {
        // What `TestHarness::start` actually generates: a v4 UUID, hyphens → `_`.
        let generated = format!(
            "{TEST_DB_PREFIX}{}",
            Uuid::new_v4().to_string().replace('-', "_")
        );
        assert!(is_engine_test_db_name(&generated), "{generated}");

        // The shared databases this fix must never take with it.
        assert!(!is_engine_test_db_name(
            "cytoanalyst_test_template_1a2b3c4d"
        ));
        assert!(!is_engine_test_db_name("cytoanalyst_build_1a2b3c4d"));
        assert!(!is_engine_test_db_name("ziee_test_template_1a2b3c4d"));
        assert!(!is_engine_test_db_name("postgres"));

        // Unanchored containment is not a match.
        assert!(!is_engine_test_db_name("not_a_test_db_abc"));
        assert!(!is_engine_test_db_name("x_test_db_abc"));

        // The prefix alone names no database.
        assert!(!is_engine_test_db_name(TEST_DB_PREFIX));

        // Anything outside [0-9a-f_] cannot reach DDL — quotes, semicolons,
        // whitespace and the wildcards a LIKE pattern would have let through.
        for hostile in [
            "test_db_a; DROP DATABASE postgres",
            "test_db_\"a\"",
            "test_db_a b",
            "test_db_a%",
            "test_db_ZZZ",
        ] {
            assert!(!is_engine_test_db_name(hostile), "admitted {hostile:?}");
        }
    }
}
