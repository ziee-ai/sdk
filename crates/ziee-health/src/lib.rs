//! `ziee-health` — the DB-free, domain-agnostic liveness/readiness surface
//! (Chunk `health`, build-DB-free).
//!
//! Moved verbatim from ziee's `modules/health`: the `HealthResponse` wire type
//! (`types`), the pure unauthenticated `health_check` handler + its aide docs
//! (`handlers`), and the `routes()` builder that mounts `GET /health`. No DB, no
//! auth, no app-concrete types — the whole module body is liftable; only the
//! `AppModule`/`MODULE_ENTRIES` registration (which names ziee's `module_api`)
//! stays in ziee.
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision N2):
//! `health/mod.rs` re-exports `routes`/`handlers`/`types` from here, so the
//! `AppModule::register_routes` call to `routes()` resolves unchanged and the
//! schemars schema key `HealthResponse` (short ident) is byte-identical in the
//! emitted OpenAPI.

pub mod handlers;
pub mod routes;
pub mod types;

pub use routes::routes;
