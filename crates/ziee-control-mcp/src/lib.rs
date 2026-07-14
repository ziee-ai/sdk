//! `ziee-control-mcp` — the LLM-control surface, DB-free tool-dispatch core
//! (Chunk C1, build-DB-free).
//!
//! Moved verbatim from ziee's `modules/control_mcp`: the OpenAPI→operation
//! catalog ingest (`catalog::init_from_openapi` + the pure `build_catalog`), the
//! deployment-invariant reachability/mutation `policy` (secret-body denylist,
//! path-prefix/segment denials, SSE/byte-stream/token guards), and the static
//! `tools` descriptors for the three control tools
//! (`list_capabilities`/`describe_capability`/`invoke_capability`).
//!
//! Per decisions N1/N5 these are the ONLY pieces extracted in v1 — they read the
//! in-memory OpenAPI catalog and describe/classify operations without touching a
//! database. The app-side glue (the JSON-RPC `handlers` that forward the caller's
//! JWT over loopback + apply the per-user permission filter, `routes`, the
//! `mcp_servers`-row `repository`, and the `chat_extension`) STAYS in ziee: a
//! fresh app can't self-expose control until the Tier-1 `mcp` registry (v1.5).
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision N2):
//! `control_mcp/mod.rs` re-exports `catalog` / `policy` / `tools` from here, so
//! `super::catalog::…`, `super::policy::…`, `super::tools::…` in the retained
//! app-side `handlers.rs` — and `control_mcp::catalog::init_from_openapi` at the
//! two boot sites — resolve unchanged.

pub mod catalog;
pub mod policy;
pub mod tools;
