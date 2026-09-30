//! The template cache is keyed on the migration set (owner card
//! `tenant-boundary-migration-dirs-pin`, memo integration-contracts-1043 §1).
//!
//! Before: one process-wide `OnceCell<()>` — the first migration set to build a
//! template decided the schema for every later caller in the process, so a test
//! binary could never get a second, differently-migrated template from the
//! harness. After: each distinct ordered set of migration dirs gets its own
//! template, built once; the app's own set keeps its historical template.
//!
//! Needs the shared Postgres (`DATABASE_URL`, default 127.0.0.1:54321) — the
//! same cluster every harness consumer runs against. No app binary is spawned:
//! [`TestHarness::template_for`] is the template-acquisition API under test.

use std::path::{Path, PathBuf};

use sqlx::postgres::PgPoolOptions;
use ziee_test_harness::{HarnessApp, SpawnFacts, SpawnPlan, TestHarness, Variant};

fn admin_url() -> String {
    std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgresql://postgres:password@127.0.0.1:54321/postgres".to_string())
}

fn block_on<F: std::future::Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(f)
}

/// A harness app whose own migration set is `default_dirs`, under a template
/// base unique to one test (so tests in this binary never share a template).
struct SetApp {
    base: String,
    default_dirs: Vec<PathBuf>,
}

impl HarnessApp for SetApp {
    type Options = ();
    fn template_db_base(&self, _: Variant) -> String {
        self.base.clone()
    }
    fn migration_dirs(&self, _: Variant, _: &Path) -> Vec<PathBuf> {
        self.default_dirs.clone()
    }
    fn plan_spawn(&self, _: &(), _: &SpawnFacts) -> SpawnPlan {
        unreachable!("these tests acquire templates; they never spawn a server")
    }
}

/// A migration dir holding one migration that creates `table`.
fn migration_dir(root: &Path, name: &str, version: u32, table: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{version:04}_create_{table}.sql")),
        format!("CREATE TABLE {table} (id INT PRIMARY KEY);"),
    )
    .unwrap();
    dir
}

fn harness(base: &str, default_dirs: Vec<PathBuf>) -> TestHarness<SetApp> {
    TestHarness::new(
        SetApp {
            base: base.to_string(),
            default_dirs,
        },
        PathBuf::from(env!("CARGO_MANIFEST_DIR")),
        Variant::Server,
    )
}

fn unique_base() -> String {
    format!("zth_selftest_{}", &uuid::Uuid::new_v4().simple().to_string()[..8])
}

/// The public tables of database `db`, sorted.
async fn tables_of(db: &str) -> Vec<String> {
    let mut url = url::Url::parse(&admin_url()).unwrap();
    url.set_path(db);
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(url.as_str())
        .await
        .unwrap_or_else(|e| panic!("connect template {db}: {e}"));
    let mut rows: Vec<(String,)> = sqlx::query_as(
        "SELECT table_name::text FROM information_schema.tables \
          WHERE table_schema = 'public' AND table_name <> '_sqlx_migrations'",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    pool.close().await;
    rows.sort();
    rows.into_iter().map(|(t,)| t).collect()
}

/// The template's `pg_database.oid` — changes iff it was dropped and rebuilt.
async fn oid_of(db: &str) -> Option<u32> {
    let pool = PgPoolOptions::new().max_connections(1).connect(&admin_url()).await.unwrap();
    let row: Option<(sqlx::postgres::types::Oid,)> =
        sqlx::query_as("SELECT oid FROM pg_database WHERE datname = $1")
            .bind(db)
            .fetch_optional(&pool)
            .await
            .unwrap();
    pool.close().await;
    row.map(|(o,)| o.0)
}

/// Drop the templates a test built (they are otherwise rebuilt per process, but
/// these self-test names are unique per run and would accumulate).
async fn drop_templates(names: &[&str]) {
    let pool = PgPoolOptions::new().max_connections(1).connect(&admin_url()).await.unwrap();
    for n in names {
        let _ = sqlx::query(&format!("DROP DATABASE IF EXISTS {n} WITH (FORCE)"))
            .execute(&pool)
            .await;
    }
    pool.close().await;
}

/// A process needing two different migration sets gets two templates, each
/// migrated from its own set, and neither overwrites the other.
#[test]
fn two_migration_sets_get_two_templates() {
    block_on(async {
        let root = tempfile::tempdir().unwrap();
        let alpha = migration_dir(root.path(), "alpha", 1, "alpha");
        let beta = migration_dir(root.path(), "beta", 2, "beta");
        let app_set = vec![alpha.clone()];
        let fixture_set = vec![alpha.clone(), beta.clone()];
        let h = harness(&unique_base(), app_set.clone());

        let t_app = h.template_for(&app_set).await;
        let app_oid = oid_of(&t_app).await;
        let t_fixture = h.template_for(&fixture_set).await;

        assert_ne!(t_app, t_fixture, "two migration sets → two template databases");
        assert_eq!(tables_of(&t_app).await, vec!["alpha"], "the app set's schema");
        assert_eq!(
            tables_of(&t_fixture).await,
            vec!["alpha", "beta"],
            "the second set's template is migrated from the SECOND set, not handed the first"
        );

        // Asking for the first set again returns the same template, untouched by
        // the second build (not rebuilt: same oid; not overwritten: same schema).
        assert_eq!(h.template_for(&app_set).await, t_app);
        assert_eq!(oid_of(&t_app).await, app_oid, "built once per process per set");
        assert_eq!(tables_of(&t_app).await, vec!["alpha"]);

        drop_templates(&[&t_app, &t_fixture]).await;
    });
}

/// The existing single-set behaviour: the app's own set gets the historical
/// template name (`<base><worktree suffix>`), carries that set's schema, and
/// is built once — repeated acquisition neither renames nor rebuilds it.
#[test]
fn the_apps_own_set_keeps_its_historical_single_template() {
    block_on(async {
        let root = tempfile::tempdir().unwrap();
        let gamma = migration_dir(root.path(), "gamma", 1, "gamma");
        let base = unique_base();
        let app_set = vec![gamma];
        let h = harness(&base, app_set.clone());

        let t = h.template_for(&app_set).await;
        let manifest = env!("CARGO_MANIFEST_DIR");
        let expected = if ziee_test_harness::worktree_db::should_auto_isolate(
            &std::env::var("DATABASE_URL").ok(),
        ) {
            format!("{base}_{}", ziee_test_harness::worktree_db::worktree_key(manifest))
        } else {
            base.clone()
        };
        assert_eq!(t, expected, "the app's own set keeps the historical template name");
        assert_eq!(tables_of(&t).await, vec!["gamma"]);

        let oid = oid_of(&t).await;
        assert!(oid.is_some(), "the template exists");
        for _ in 0..3 {
            assert_eq!(h.template_for(&app_set).await, t);
        }
        assert_eq!(oid_of(&t).await, oid, "one build per process, however often it is asked for");

        drop_templates(&[&t]).await;
    });
}
