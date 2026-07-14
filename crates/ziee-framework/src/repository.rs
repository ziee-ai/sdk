//! Repository Factory — generic `Repos.module.method()` access machinery.
//!
//! This module hosts ONLY the app-agnostic `declare_repositories!` macro (moved
//! out of the ziee server crate in Chunk B4). The macro is generic over a list
//! of `(field, RepositoryType => module_path)` triples; its expansion generates
//! the `RepositoryFactory`, the per-type `Deref` wrappers, the `ReposAccessor`,
//! the `Repos` global const, and the `init_repositories` / `is_repos_initialized`
//! codegen — ALL in the INVOKING crate (so `Repos` lives app-side).
//!
//! The concrete repo LIST + the `Repos.xxx` call sites stay in ziee: ziee invokes
//! `ziee_framework::declare_repositories! { user: UserRepository => crate::modules::user, ... }`.
//!
//! Cross-crate hygiene: every path the expansion relies on is FULLY-QUALIFIED
//! (`::once_cell`, `::sqlx`, `::std`, `::paste`, `::tracing`) so it resolves in
//! any invoking crate regardless of that crate's `use` imports. The invoking
//! crate must have those crates as dependencies (ziee does). The `$module_path`
//! and `$type` tokens come from the invocation, so their `crate::` prefixes
//! resolve against the invoking crate — exactly as when the macro lived there.

/// Declarative macro for repository registration
///
/// This macro generates all the boilerplate needed for repository access:
/// - Imports
/// - RepositoryFactory struct with fields
/// - Getter methods
/// - Wrapper structs with Deref implementations
/// - ReposAccessor struct
/// - Repos constant
/// - Initialization functions
///
/// Reduces ~23 lines of boilerplate per repository to just 1 line.
#[macro_export]
macro_rules! declare_repositories {
    ($(
        $field:ident: $type:ident => $module_path:path
    ),* $(,)?) => {
        // ============================================
        // SECTION 1: IMPORTS
        // ============================================
        $(
            use $module_path::{ $type };
        )*

        // ============================================
        // SECTION 2: REPOSITORY FACTORY
        // ============================================
        pub struct RepositoryFactory {
            pool: ::sqlx::PgPool,
            $(
                $field: ::once_cell::sync::OnceCell<::std::sync::Arc<$type>>,
            )*
        }

        impl RepositoryFactory {
            fn new(pool: ::sqlx::PgPool) -> Self {
                Self {
                    pool,
                    $(
                        $field: ::once_cell::sync::OnceCell::new(),
                    )*
                }
            }

            pub fn pool(&self) -> &::sqlx::PgPool {
                &self.pool
            }

            $(
                pub fn $field(&self) -> &::std::sync::Arc<$type> {
                    self.$field
                        .get_or_init(|| ::std::sync::Arc::new($type::new(self.pool.clone())))
                }
            )*
        }

        // ============================================
        // SECTION 2.5: FACTORY INITIALIZATION
        // ============================================
        // The factory is stored as a leaked `&'static` behind an RwLock so
        // that `get_factory()` (and therefore `Repos.*` Deref + `pool()`)
        // can keep returning `&'static` references with zero per-call
        // allocation, while still allowing the global to be OVERWRITTEN.
        //
        // Production calls `init_repositories` exactly once at boot, so the
        // leak is a single factory for the process lifetime. The integration
        // test binary calls it once per `TestServer::start` (hundreds of
        // times per process) — each re-init leaks one small factory, which
        // is bounded and acceptable for a test process. Crucially, re-init
        // now actually SWAPS the active pool (the old `OnceCell` set-once
        // silently kept the first pool, so every later in-process-`Repos`
        // test operated on an already-dropped test DB → spurious
        // "not initialized" / duplicate-key / missing-relation failures).
        static FACTORY: ::std::sync::RwLock<Option<&'static RepositoryFactory>> =
            ::std::sync::RwLock::new(None);

        /// Initialize (or re-initialize) the global repository factory.
        ///
        /// Overwrites any previously-installed factory. In a non-test build
        /// a second call is logged as a warning (it signals a second
        /// bootstrap path in production), but the overwrite still happens.
        pub fn init_repositories(pool: ::sqlx::PgPool) {
            let leaked: &'static RepositoryFactory =
                Box::leak(Box::new(RepositoryFactory::new(pool)));
            let mut guard = FACTORY.write().unwrap_or_else(|e| e.into_inner());
            #[cfg(not(test))]
            if guard.is_some() {
                ::tracing::warn!(
                    "init_repositories called more than once in this process; \
                     overwriting the active factory. In production this signals \
                     a second bootstrap path — investigate."
                );
            }
            *guard = Some(leaked);
        }

        fn get_factory() -> &'static RepositoryFactory {
            FACTORY
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .expect("RepositoryFactory not initialized. Call init_repositories() first.")
        }

        /// True if `init_repositories()` has been called and the
        /// global factory is set. Useful for code paths that fire
        /// before the server's startup sequence has finished (e.g.
        /// the desktop tauri crate's auto_start tunnel hook, which
        /// races against the embedded-PostgreSQL boot).
        pub fn is_repos_initialized() -> bool {
            FACTORY.read().unwrap_or_else(|e| e.into_inner()).is_some()
        }

        // ============================================
        // SECTION 3: WRAPPER STRUCTS (Deref pattern)
        // ============================================
        ::paste::paste! {
            $(
                pub struct [<$type Repos>];
                impl ::std::ops::Deref for [<$type Repos>] {
                    type Target = ::std::sync::Arc<$type>;
                    fn deref(&self) -> &Self::Target {
                        // Resolve against the CURRENT factory on every access.
                        // A previous version cached the Arc in a per-type
                        // `static INSTANCE`, which pinned the first factory's
                        // pool forever and defeated re-init (the integration
                        // tests re-init per `TestServer::start`).
                        get_factory().$field()
                    }
                }
            )*
        }

        // ============================================
        // SECTION 4: REPOS ACCESSOR
        // ============================================
        ::paste::paste! {
            pub struct ReposAccessor {
                $(
                    pub $field: [<$type Repos>],
                )*
            }
        }

        impl ReposAccessor {
            /// Get the underlying database pool
            ///
            /// For use by modules that haven't been migrated to repository pattern yet.
            /// Prefer using specific repository methods when available.
            pub fn pool(&self) -> &::sqlx::PgPool {
                get_factory().pool()
            }
        }

        // ============================================
        // SECTION 5: GLOBAL CONSTANT
        // ============================================
        ::paste::paste! {
            /// Global repository accessor
            ///
            /// Provides direct field access to all repositories. All repositories are
            /// lazily initialized and cached for the lifetime of the application.
            ///
            /// # Example
            /// ```
            /// let users = Repos.user.find_all().await?;
            /// let user = Repos.user.find_by_id(user_id).await?;
            /// let groups = Repos.group.find_by_user_id(user_id).await?;
            /// let pool = Repos.pool(); // For modules without repository structs
            /// ```
            #[allow(non_upper_case_globals)]
            pub const Repos: ReposAccessor = ReposAccessor {
                $(
                    $field: [<$type Repos>],
                )*
            };
        }
    };
}
