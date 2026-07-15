//! Tool implementations dispatched by the ziee server's
//! `handlers::jsonrpc_handler`. The engine owns `execute` (the sandboxed
//! `execute_command` path incl. the pure per-OS `apply_workspace_mode` chmod);
//! `files` stays in the ziee server crate (DB-backed workspace file ops).

pub mod execute;
