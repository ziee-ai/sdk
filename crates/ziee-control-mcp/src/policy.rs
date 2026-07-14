//! The control-surface policy gate: which operations are reachable at all, and
//! which are state-changing.
//!
//! Excluded classes (never drivable, in addition to the auth/test/health/
//! server-update prefixes): MCP JSON-RPC endpoints (no recursion), SSE streams
//! (would hang the dispatch), raw byte-stream GETs (download/content/preview/
//! thumbnail/artifact/export), token/credential minting, and — crucially — any
//! op whose REQUEST BODY carries a secret (api_key/password/client_secret/…),
//! since driving it would persist the plaintext into the conversation. A
//! consequence: e.g. `User.create` (needs a password) is NOT drivable — use the
//! UI for secret-bearing writes.
//!
//! Security posture (see the plan): expose EVERYTHING except a hard denylist.
//! Permission checks (per-user) happen elsewhere; this module is the
//! deployment-invariant "some operations must never be driven by the model"
//! layer — auth token flows, test scaffolding, SSE streams, raw byte streams,
//! self-recursion into any MCP endpoint, and the server self-update apply.

use super::catalog::Operation;

/// Anything that is not a GET is treated as state-changing (forces approval).
pub fn is_mutating(method: &str) -> bool {
    !method.eq_ignore_ascii_case("GET")
}

/// Exact path prefixes that are never drivable. Matched against the operation's
/// `path_template` (which includes the `/api` prefix).
const DENY_PATH_PREFIXES: &[&str] = &[
    // Auth token flows — the model must never mint/rotate/destroy sessions.
    "/api/auth/login",
    "/api/auth/register",
    "/api/auth/refresh",
    "/api/auth/logout",
    "/api/auth/oauth",
    // Test-only scaffolding.
    "/api/_test",
    // Liveness (nothing to do; also unauthenticated).
    "/api/health",
    // Server self-update apply (irreversible, host-level).
    "/api/server-update",
    // Auth-provider (OIDC/OAuth/LDAP) admin config: the `config` body is a
    // free-form object whose schema can't expose the nested client_secret by
    // name, so the secret-field rule can't see it — deny the whole surface.
    "/api/admin/auth-providers",
];

/// Path SEGMENTS (exact, `/`-delimited) that are never drivable, wherever they
/// appear in the path. Segment-exact matching (not raw substring) so we deny the
/// MCP JSON-RPC endpoints (`.../mcp`) and byte streams (`.../download`,
/// `.../content`) without also hiding legitimate management routes like
/// `/api/mcp-servers/...`, `/api/downloads/...`, or `.../table-of-contents` (L5).
/// SSE stream segments (any position) — an SSE response would hang the loopback
/// dispatch to its timeout. The `-stream`/`-events` suffix rule also catches
/// compound spellings like `usage-stream` (aide reports these as
/// `application/json` in the spec, so we can't detect them by content-type).
fn is_sse_segment(seg: &str) -> bool {
    matches!(seg, "subscribe" | "stream" | "events")
        || seg.ends_with("-stream")
        || seg.ends_with("-events")
}

/// Token / credential-minting segments (any position) — a model must not mint
/// or rotate bearer/download/proxy tokens even if the user could. Suffix match
/// so compound spellings (`download-token`, `rotate-proxy-token`) are caught.
fn is_token_segment(seg: &str) -> bool {
    seg == "tokens" || seg.ends_with("token") || seg.ends_with("api-keys") || seg == "api-keys"
}

/// Raw file BYTE-stream segments — only meaningful on a GET (the response is
/// binary/text, useless as a JSON tool result). Position-independent but
/// GET-gated so that POST *actions* like `POST /api/llm-models/download`
/// (a "start a download" command that returns JSON) stay drivable.
fn is_byte_stream_segment(seg: &str) -> bool {
    matches!(
        seg,
        "download" | "content" | "preview" | "thumbnail" | "artifact" | "export"
    )
}

/// True when the operation must never be exposed to the control tools.
pub fn is_denied(op: &Operation) -> bool {
    let path = op.path_template.as_str();

    if DENY_PATH_PREFIXES.iter().any(|p| path.starts_with(p)) {
        return true;
    }

    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // MCP JSON-RPC recursion: ONLY the built-in `/api/<x>/mcp` endpoints, i.e.
    // `mcp` as the LAST segment. Management routes under `/api/mcp/...` (add /
    // list / configure MCP servers) are legitimate control targets and stay
    // drivable.
    if segments.last() == Some(&"mcp") {
        return true;
    }
    // SSE streams (hang) — matched on the LAST segment only: every SSE endpoint
    // terminates in the stream word (`.../stream`, `.../subscribe`, `.../events`,
    // `.../usage-stream`), while a JSON config route like
    // `PUT /api/chat/stream/subscription` has `stream` mid-path and must stay
    // drivable.
    if segments.last().is_some_and(|last| is_sse_segment(last)) {
        return true;
    }
    // Token / credential minting — any position.
    if segments.iter().any(|s| is_token_segment(s)) {
        return true;
    }
    // Raw byte streams — GET only (POST download-*actions* return JSON, keep them).
    if op.method.eq_ignore_ascii_case("GET")
        && segments.iter().any(|s| is_byte_stream_segment(s))
    {
        return true;
    }
    // Secret-bearing request body — driving it would persist the plaintext
    // secret (api_key / password / client_secret / …) into the conversation's
    // tool-call arguments. General rule; catches provider-key / auth-provider /
    // password writes wherever they live.
    if op.has_secret_field {
        return true;
    }
    // A state-changing operation with a non-JSON body (multipart upload /
    // octet-stream) can't be driven by a JSON tool call — deny rather than
    // dispatch a malformed request.
    if is_mutating(&op.method) && !op.json_body {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn op(operation_id: &str, method: &str, path: &str, json_body: bool) -> Operation {
        Operation {
            operation_id: operation_id.to_string(),
            method: method.to_string(),
            path_template: path.to_string(),
            tags: vec![],
            summary: String::new(),
            required_permission: None,
            path_params: vec![],
            request_schema: None,
            json_body,
            has_secret_field: false,
            parameters: Vec::<Value>::new(),
        }
    }

    fn secret_op(operation_id: &str, method: &str, path: &str) -> Operation {
        let mut o = op(operation_id, method, path, true);
        o.has_secret_field = true;
        o
    }

    #[test]
    fn is_mutating_only_non_get() {
        assert!(!is_mutating("GET"));
        assert!(!is_mutating("get"));
        assert!(is_mutating("POST"));
        assert!(is_mutating("PUT"));
        assert!(is_mutating("DELETE"));
        assert!(is_mutating("PATCH"));
    }

    #[test]
    fn denylist_covers_every_category() {
        // Auth token flows.
        assert!(is_denied(&op("Auth.login", "POST", "/api/auth/login", true)));
        assert!(is_denied(&op("Auth.refresh", "POST", "/api/auth/refresh", true)));
        assert!(is_denied(&op("Auth.logout", "POST", "/api/auth/logout", true)));
        assert!(is_denied(&op("Auth.register", "POST", "/api/auth/register", true)));
        // Test scaffolding.
        assert!(is_denied(&op("Test.seed", "POST", "/api/_test/seed", true)));
        // Health.
        assert!(is_denied(&op("Health.check", "GET", "/api/health", true)));
        // Sync SSE.
        assert!(is_denied(&op("Sync.subscribe", "GET", "/api/sync/subscribe", true)));
        // Server-update apply.
        assert!(is_denied(&op("ServerUpdate.apply", "POST", "/api/server-update/apply", true)));
        // MCP JSON-RPC recursion (control itself + siblings) — `mcp` as LAST segment.
        assert!(is_denied(&op("Control.rpc", "POST", "/api/control/mcp", true)));
        assert!(is_denied(&op("Files.rpc", "POST", "/api/files/mcp", true)));
        // Raw byte streams (GET).
        assert!(is_denied(&op("File.download", "GET", "/api/files/{id}/download", true)));
        assert!(is_denied(&op("File.content", "GET", "/api/files/{id}/content", true)));
        assert!(is_denied(&op("File.preview", "GET", "/api/files/{id}/preview", true)));
        assert!(is_denied(&op("File.thumbnail", "GET", "/api/files/{id}/thumbnail", true)));
        // EVERY SSE stream endpoint must be denied (they'd hang the dispatch).
        assert!(is_denied(&op("ChatStream.subscribe", "GET", "/api/chat/stream", true)));
        assert!(is_denied(&op("CodeSandbox.subscribeRootfsInstallProgress", "GET", "/api/code-sandbox/rootfs/versions/install/subscribe", true)));
        assert!(is_denied(&op("Hardware.stream", "GET", "/api/hardware/usage-stream", true)));
        assert!(is_denied(&op("LlmModel.subscribeDownloadProgress", "GET", "/api/llm-models/downloads/subscribe", true)));
        assert!(is_denied(&op("LocalRuntime.streamLogs", "GET", "/api/local-runtime/models/{id}/logs/stream", true)));
        assert!(is_denied(&op("RuntimeVersion.subscribeDownloadEvents", "GET", "/api/local-runtime/versions/downloads/{key}/events", true)));
        assert!(is_denied(&op("Sync.subscribe", "GET", "/api/sync/subscribe", true)));
        assert!(is_denied(&op("Workflow.subscribeRunEvents", "GET", "/api/workflow-runs/{run_id}/events", true)));
        // Token / credential minting (incl. compound `rotate-proxy-token`) + api-keys.
        assert!(is_denied(&op("File.generateDownloadToken", "POST", "/api/files/{id}/download-token", true)));
        assert!(is_denied(&op("File.downloadWithToken", "GET", "/api/files/{id}/download-with-token", true)));
        assert!(is_denied(&op("LlmProvider.rotateProxyToken", "POST", "/api/llm-providers/{id}/rotate-proxy-token", true)));
        assert!(is_denied(&op("LlmProvider.saveUserApiKey", "POST", "/api/user-llm-providers/api-keys", true)));
        assert!(is_denied(&op("LlmProvider.listUserApiKeys", "GET", "/api/user-llm-providers/api-keys", true)));
        // Raw byte artifact + export (GET).
        assert!(is_denied(&op("Workflow.readArtifact", "GET", "/api/workflow-runs/{run_id}/artifact/{step_id}/{filename}", true)));
        assert!(is_denied(&op("Citations.export", "GET", "/api/citations/export", true)));
        // Secret-bearing request body (persisting plaintext into tool args).
        assert!(is_denied(&secret_op("LlmProvider.create", "POST", "/api/llm-providers")));
        assert!(is_denied(&secret_op("AuthProviders.create", "POST", "/api/admin/auth-providers")));
        assert!(is_denied(&secret_op("Auth.changePassword", "POST", "/api/auth/password")));
        // Multipart upload (mutating, non-JSON body).
        assert!(is_denied(&op("File.upload", "POST", "/api/files", false)));
    }

    #[test]
    fn normal_operations_are_allowed() {
        assert!(!is_denied(&op("User.create", "POST", "/api/users", true)));
        assert!(!is_denied(&op("User.list", "GET", "/api/users", true)));
        assert!(!is_denied(&op("Assistant.update", "PUT", "/api/assistants/{id}", true)));
        assert!(!is_denied(&op("Assistant.delete", "DELETE", "/api/assistants/{id}", true)));
    }

    #[test]
    fn segment_matching_does_not_overreach() {
        // Management routes that merely CONTAIN a denied word must stay drivable.
        assert!(!is_denied(&op("McpServer.list", "GET", "/api/mcp-servers", true)));
        assert!(!is_denied(&op("Download.history", "GET", "/api/downloads", true)));
        assert!(!is_denied(&op("Doc.toc", "GET", "/api/docs/{id}/table-of-contents", true)));
        // `/api/mcp/*` MANAGEMENT routes (mcp NOT last segment) are legitimate
        // control targets — only the built-in `.../mcp` JSON-RPC endpoints are denied.
        assert!(!is_denied(&op("McpServer.list", "GET", "/api/mcp/servers", true)));
        assert!(!is_denied(&op("McpServer.create", "POST", "/api/mcp/servers", true)));
        assert!(!is_denied(&op("Mcp.userPolicy", "GET", "/api/mcp/user-policy", true)));
        // POST "download" ACTIONS (return JSON, not bytes) stay drivable; only
        // GET byte-downloads are denied.
        assert!(!is_denied(&op("LlmModel.download", "POST", "/api/llm-models/download", true)));
        assert!(!is_denied(&op("RuntimeVersion.download", "POST", "/api/local-runtime/versions/download", true)));
        // A JSON config write whose path merely CONTAINS `stream` mid-path is not
        // an SSE stream — stays drivable (last-segment SSE rule).
        assert!(!is_denied(&op("ChatStream.setSubscription", "PUT", "/api/chat/stream/subscription", true)));
    }
}
