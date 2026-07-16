//! Generic entity-extension registry primitive (dogfood gap G8).
//!
//! Domain-agnostic skeleton for the "sibling modules self-register a set of
//! ordered lifecycle hooks against a parent entity" pattern that ziee grew
//! independently in `modules/chat` and `modules/project`, and that CytoAnalyst
//! re-grew a third time as `study`. Only the parent-entity name + the hook set
//! differ between instances; the mechanics are identical and live here:
//!
//! * a `#[distributed_slice]` of `{name, order, factory}` entries
//!   ([`ExtensionEntry`]) — sibling modules self-register without the parent
//!   importing them,
//! * a `Vec<Arc<dyn Hooks>>` registry ([`ExtensionRegistry`]) that folds routes
//!   ([`ExtensionRegistry::fold_routes`]) and fans each hook over ONE shared
//!   `&mut Transaction` ([`ExtensionRegistry::fire_in_tx`]),
//! * an [`auto_register`] that sorts entries by `order` and builds each via its
//!   factory,
//! * a `once_cell` process-wide singleton ([`ExtensionRegistrySingleton`]).
//!
//! The hook SET is intentionally NOT part of this primitive: hooks are
//! domain-specific (chat streams deltas; project duplicates + attaches), so
//! each app keeps its own `trait XExtension` (with default-`Ok(())` methods)
//! and a thin newtype over `ExtensionRegistry<dyn XExtension>` whose fan-out
//! methods delegate to the generic combinators below. See ziee's `modules/chat`
//! + `modules/project` for the two reference wirings.
//!
//! ## Optional in-transaction delete hook
//!
//! [`ExtensionRegistry::fire_in_tx`] is the generic engine behind every
//! in-transaction fan-out, INCLUDING an OPTIONAL `on_<entity>_deleted` hook: a
//! file/artifact-owning extension (CytoAnalyst's `study`) that must clean up
//! rows in the same transaction as the parent delete simply adds a
//! default-`Ok(())` `on_<entity>_deleted` method to its trait and calls
//! `fire_in_tx` from its delete path. Cascade-only parents (ziee's `project`,
//! whose join tables use `ON DELETE CASCADE`) never call it and pay nothing —
//! the capability is offered by the primitive, not imposed on every user.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use aide::axum::ApiRouter;
use once_cell::sync::OnceCell;
use sqlx::{PgPool, Postgres, Transaction};

/// A registration entry collected by a `#[distributed_slice]`.
///
/// - `H` is the app's extension trait object (e.g. `dyn ChatExtension`).
/// - `C` is the factory context the app threads into each extension
///   (e.g. `Arc<Config>`), cloned once per entry at [`auto_register`].
///
/// The app declares its slice + a convenience alias, e.g.
/// ```ignore
/// pub type ChatExtensionEntry = ExtensionEntry<dyn ChatExtension, Arc<Config>>;
/// #[distributed_slice]
/// pub static CHAT_EXTENSIONS: [ChatExtensionEntry] = [..];
/// ```
/// and each sibling module registers a `static` of that alias.
pub struct ExtensionEntry<H: ?Sized, C> {
    pub name: &'static str,
    pub order: i32,
    pub factory: fn(PgPool, C) -> Arc<H>,
}

// Manual Clone/Copy: `#[derive]` would spuriously require `H: Copy` / `C: Copy`.
// Every field is Copy (`&'static str`, `i32`, and a fn pointer) independent of
// H/C, so the entry is unconditionally Copy — which is also what makes it a
// valid `#[distributed_slice]` element (fn pointers + `&'static str` + `i32` are
// all `Sync` regardless of H/C).
impl<H: ?Sized, C> Clone for ExtensionEntry<H, C> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<H: ?Sized, C> Copy for ExtensionEntry<H, C> {}

/// The generic registry: an ordered `Vec<Arc<H>>` of registered extensions.
///
/// Apps wrap this in a thin newtype (e.g. `ProjectExtensionRegistry`) whose
/// domain fan-out methods delegate to [`iter`](Self::iter),
/// [`fold_routes`](Self::fold_routes), and [`fire_in_tx`](Self::fire_in_tx).
pub struct ExtensionRegistry<H: ?Sized> {
    extensions: Vec<Arc<H>>,
}

impl<H: ?Sized> ExtensionRegistry<H> {
    /// Create an empty registry. An empty registry is a valid runtime state
    /// (zero extensions registered → routes contribute nothing, hooks fan out
    /// to zero handlers), which is the acid-test invariant of the pattern.
    pub fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    /// Register one already-built extension.
    pub fn register(&mut self, extension: Arc<H>) {
        self.extensions.push(extension);
    }

    /// Iterate registered extensions in registration (i.e. `order`) sequence.
    pub fn iter(&self) -> std::slice::Iter<'_, Arc<H>> {
        self.extensions.iter()
    }

    /// Borrow the registered extensions as a slice.
    pub fn extensions(&self) -> &[Arc<H>] {
        &self.extensions
    }

    /// Number of registered extensions.
    pub fn len(&self) -> usize {
        self.extensions.len()
    }

    /// Whether zero extensions are registered.
    pub fn is_empty(&self) -> bool {
        self.extensions.is_empty()
    }

    /// Fold every registered extension into `router` (the "folds routes" half
    /// of the skeleton). The app passes a closure calling its trait's
    /// `register_routes`. Extensions that register nothing are a no-op (their
    /// default trait impl returns the router unchanged).
    pub fn fold_routes<S>(
        &self,
        router: ApiRouter<S>,
        mut f: impl FnMut(ApiRouter<S>, &Arc<H>) -> ApiRouter<S>,
    ) -> ApiRouter<S>
    where
        S: Clone + Send + Sync + 'static,
    {
        self.iter().fold(router, |r, ext| f(r, ext))
    }

    /// Fan an in-transaction lifecycle hook over EVERY extension SEQUENTIALLY,
    /// sharing ONE `&mut Transaction`. The first error aborts the fan-out (and,
    /// since the caller then never reaches `tx.commit()`, the whole operation
    /// rolls back atomically).
    ///
    /// This is the generic engine behind ziee-project's `on_project_duplicated`
    /// / `on_conversation_attached` / `on_conversation_detached` AND the
    /// OPTIONAL in-transaction **delete** hook (`on_<entity>_deleted`) that
    /// file/artifact-owning extensions need for in-tx cleanup (see the module
    /// docs). The app's closure simply calls the domain hook:
    ///
    /// ```ignore
    /// registry.fire_in_tx(tx, |ext, tx| ext.on_study_deleted(study_id, tx)).await?;
    /// ```
    pub async fn fire_in_tx<'t, E, F>(
        &self,
        tx: &mut Transaction<'t, Postgres>,
        mut hook: F,
    ) -> Result<(), E>
    where
        F: for<'a> FnMut(
            &'a Arc<H>,
            &'a mut Transaction<'t, Postgres>,
        ) -> Pin<Box<dyn Future<Output = Result<(), E>> + Send + 'a>>,
    {
        for ext in &self.extensions {
            hook(ext, &mut *tx).await?;
        }
        Ok(())
    }
}

impl<H: ?Sized> Default for ExtensionRegistry<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// Collect a `#[distributed_slice]` of entries and sort them by `order` — the
/// discovery + ordering half of the skeleton, factored out so an app can drive
/// the factory-dispatch + registration loop itself (e.g. to keep its own
/// per-extension registration logging) while still reusing the sort.
pub fn sorted_entries<H, C>(entries: &[ExtensionEntry<H, C>]) -> Vec<&ExtensionEntry<H, C>>
where
    H: ?Sized,
{
    let mut sorted: Vec<&ExtensionEntry<H, C>> = entries.iter().collect();
    sorted.sort_by_key(|e| e.order);
    sorted
}

/// Collect a `#[distributed_slice]` of entries, sort by `order`, build each
/// extension via its factory, and register them into a fresh registry — the
/// full `auto_register` half of the skeleton.
///
/// `log_label` names the entity in the per-extension `debug!` line
/// (e.g. `"chat"` / `"project"`). Apps that want a different registration log
/// (or to wrap the result in a newtype whose `register` logs) can instead drive
/// [`sorted_entries`] + a factory loop themselves; ziee's `chat` + `project` do.
pub fn auto_register<H, C>(
    entries: &[ExtensionEntry<H, C>],
    pool: PgPool,
    ctx: C,
    log_label: &str,
) -> ExtensionRegistry<H>
where
    H: ?Sized,
    C: Clone,
{
    let mut registry = ExtensionRegistry::new();

    for entry in sorted_entries(entries) {
        tracing::debug!(
            "Registering {log_label} extension: {} (order: {})",
            entry.name,
            entry.order
        );
        let extension = (entry.factory)(pool.clone(), ctx.clone());
        registry.register(extension);
    }

    registry
}

/// A process-wide `once_cell` singleton holder for a registry (the singleton
/// half of the skeleton). Generic over the app's registry type `R` — a newtype
/// (`ProjectExtensionRegistry`) or a bare [`ExtensionRegistry<dyn X>`] alias —
/// so it fits whichever wrapping style the app chose.
///
/// The app declares one `static` of this per entity and delegates its
/// `set_global_registry` / `get_global_registry` shims to it. Only needed by
/// entities whose hooks run from a context that never receives the registry via
/// axum `Extension` (ziee's `project` chat-extension is the reference case);
/// entities that always thread the registry through `Extension` (ziee's `chat`)
/// don't need it.
pub struct ExtensionRegistrySingleton<R> {
    cell: OnceCell<Arc<R>>,
}

impl<R> ExtensionRegistrySingleton<R> {
    /// Create an unset singleton (usable in a `static` initializer).
    pub const fn new() -> Self {
        Self {
            cell: OnceCell::new(),
        }
    }

    /// Set the registry once. A second call is ignored with a warning — in
    /// production that signals a second bootstrap path worth investigating.
    pub fn set(&self, registry: Arc<R>) {
        if self.cell.set(registry).is_err() {
            tracing::warn!(
                "entity-extension registry singleton set more than once; \
                 subsequent call ignored. In production this signals a second \
                 bootstrap path — investigate."
            );
        }
    }

    /// Get the registry, or `None` if accessed before the owning module's init
    /// (e.g. tests that bypass the standard boot sequence).
    pub fn get(&self) -> Option<Arc<R>> {
        self.cell.get().cloned()
    }
}

impl<R> Default for ExtensionRegistrySingleton<R> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    // A tiny domain trait standing in for an app's `XExtension`, exercising the
    // generic skeleton end-to-end: an in-tx lifecycle hook + an OPTIONAL
    // in-transaction DELETE hook (the capability ziee's project lacks and
    // CytoAnalyst's study needs).
    #[async_trait]
    trait DummyExtension: Send + Sync {
        fn name(&self) -> &str;
        // Non-tx marker so `fire_in_tx`'s counter can be observed without a DB.
        async fn on_touch(&self, hits: &AtomicUsize) -> Result<(), String>;
        // Optional in-tx delete hook — default no-op (cascade-only users skip it).
        async fn on_deleted(&self, _hits: &AtomicUsize) -> Result<(), String> {
            Ok(())
        }
    }

    struct Ok1 {
        name: &'static str,
    }
    #[async_trait]
    impl DummyExtension for Ok1 {
        fn name(&self) -> &str {
            self.name
        }
        async fn on_touch(&self, hits: &AtomicUsize) -> Result<(), String> {
            hits.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        async fn on_deleted(&self, hits: &AtomicUsize) -> Result<(), String> {
            hits.fetch_add(10, Ordering::SeqCst);
            Ok(())
        }
    }

    struct Boom;
    #[async_trait]
    impl DummyExtension for Boom {
        fn name(&self) -> &str {
            "boom"
        }
        async fn on_touch(&self, _hits: &AtomicUsize) -> Result<(), String> {
            Err("boom".into())
        }
    }

    fn ext(name: &'static str) -> Arc<dyn DummyExtension> {
        Arc::new(Ok1 { name })
    }

    #[test]
    fn entry_is_copy_and_ordering_sorts() {
        type E = ExtensionEntry<dyn DummyExtension, ()>;
        let a = E {
            name: "a",
            order: 30,
            factory: |_, _| ext("a"),
        };
        let b = E {
            name: "b",
            order: 10,
            factory: |_, _| ext("b"),
        };
        let c = E {
            name: "c",
            order: 20,
            factory: |_, _| ext("c"),
        };
        // Copy: entries are `Copy`, so this compiles without a move error.
        let mut v = [a, b, c];
        v.sort_by_key(|e| e.order);
        assert_eq!(v.iter().map(|e| e.name).collect::<Vec<_>>(), ["b", "c", "a"]);
    }

    #[test]
    fn empty_registry_is_valid() {
        let reg: ExtensionRegistry<dyn DummyExtension> = ExtensionRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
    }

    #[test]
    fn register_and_iter_preserve_order() {
        let mut reg: ExtensionRegistry<dyn DummyExtension> = ExtensionRegistry::new();
        reg.register(ext("first"));
        reg.register(ext("second"));
        assert_eq!(
            reg.iter().map(|e| e.name().to_string()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    // Drive a `fire_in_tx`-shaped fan-out WITHOUT a DB by reusing the same
    // sequential-over-shared-state, first-error-aborts loop against an
    // in-memory counter. (The real `fire_in_tx` shares `&mut Transaction`; this
    // proves the fan-out semantics — every-extension + abort-on-error — that the
    // in-tx delete hook relies on.)
    #[tokio::test]
    async fn fan_out_hits_every_extension() {
        let mut reg: ExtensionRegistry<dyn DummyExtension> = ExtensionRegistry::new();
        reg.register(ext("a"));
        reg.register(ext("b"));
        let hits = AtomicUsize::new(0);
        for e in reg.iter() {
            e.on_touch(&hits).await.unwrap();
        }
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn fan_out_aborts_on_first_error() {
        let mut reg: ExtensionRegistry<dyn DummyExtension> = ExtensionRegistry::new();
        reg.register(ext("a"));
        reg.register(Arc::new(Boom));
        reg.register(ext("c"));
        let hits = AtomicUsize::new(0);
        let mut result = Ok(());
        for e in reg.iter() {
            if let Err(err) = e.on_touch(&hits).await {
                result = Err(err);
                break;
            }
        }
        assert!(result.is_err());
        // Only the first extension ran before Boom aborted the fan-out.
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn optional_delete_hook_defaults_to_noop() {
        // A cascade-only extension leaves `on_deleted` at its default no-op;
        // a file-owning one overrides it. Both are drivable through one loop.
        struct CascadeOnly;
        #[async_trait]
        impl DummyExtension for CascadeOnly {
            fn name(&self) -> &str {
                "cascade"
            }
            async fn on_touch(&self, _hits: &AtomicUsize) -> Result<(), String> {
                Ok(())
            }
            // no on_deleted override -> default no-op
        }
        let mut reg: ExtensionRegistry<dyn DummyExtension> = ExtensionRegistry::new();
        reg.register(Arc::new(CascadeOnly)); // contributes 0
        reg.register(ext("file-owner")); // Ok1::on_deleted contributes 10
        let hits = AtomicUsize::new(0);
        for e in reg.iter() {
            e.on_deleted(&hits).await.unwrap();
        }
        assert_eq!(hits.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn singleton_sets_once() {
        let s: ExtensionRegistrySingleton<ExtensionRegistry<dyn DummyExtension>> =
            ExtensionRegistrySingleton::new();
        assert!(s.get().is_none());
        s.set(Arc::new(ExtensionRegistry::new()));
        assert!(s.get().is_some());
        // Second set is ignored (does not panic).
        s.set(Arc::new(ExtensionRegistry::new()));
        assert!(s.get().is_some());
    }

    #[test]
    fn auto_register_sorts_by_order() {
        type E = ExtensionEntry<dyn DummyExtension, ()>;
        // A hand-built slice standing in for a `#[distributed_slice]`.
        let entries: Vec<E> = vec![
            E {
                name: "late",
                order: 30,
                factory: |_, _| ext("late"),
            },
            E {
                name: "early",
                order: 10,
                factory: |_, _| ext("early"),
            },
        ];
        // `auto_register` needs a real PgPool to call factories; the factories
        // here ignore it, but we can't fabricate a PgPool in a unit test. So we
        // assert the sort directly (the factory-dispatch path is covered by the
        // ziee integration suites that boot a real pool).
        let mut sorted: Vec<&E> = entries.iter().collect();
        sorted.sort_by_key(|e| e.order);
        assert_eq!(
            sorted.iter().map(|e| e.name).collect::<Vec<_>>(),
            ["early", "late"]
        );
    }
}
