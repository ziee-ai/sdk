//! Shared built-in-MCP-server scaffolding (Chunk C1).
//!
//! Two dependency-free pieces every in-process ("built-in") MCP server in the
//! app reuses, moved here verbatim from ziee's `code_sandbox` module so a fresh
//! app (and `ziee-control-mcp`) can stand up a loopback JSON-RPC MCP server
//! without re-implementing them:
//!
//! - the JSON-RPC 2.0 envelope types (`JsonRpcRequest` / `JsonRpcResponse` /
//!   `JsonRpcError`) + the canonical-code constructors, and
//! - `loopback_host`, which pins a built-in server's self-dial to `127.0.0.1`
//!   regardless of the operator's `server.host` (a security invariant — see the
//!   doc on the fn).
//!
//! ziee consumes these via equivalence-preserving re-export shims (decision
//! N2): `code_sandbox::types` re-exports the JSON-RPC types and
//! `code_sandbox::loopback_host` re-exports the function, so every existing
//! `code_sandbox::types::JsonRpc*` / `code_sandbox::loopback_host` call site is
//! unchanged. Build-DB-free: serde + serde_json + `ziee_core::AppError` only.

use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request envelope. The sandbox handler accepts only a
/// minimal subset: `initialize`, `tools/list`, `tools/call`.
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    #[serde(default = "default_jsonrpc")]
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

fn default_jsonrpc() -> String {
    "2.0".to_string()
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcError {
    /// JSON-RPC 2.0 standard codes (https://www.jsonrpc.org/specification#error_object).
    pub const PARSE_ERROR: i32 = -32700;
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    pub const INTERNAL: i32 = -32603;

    /// Invalid JSON was received (the payload was not parseable). Per the
    /// JSON-RPC spec this is `-32700`; the HTTP layer pairs it with 400.
    pub fn parse_error(detail: impl Into<String>) -> Self {
        Self {
            code: Self::PARSE_ERROR,
            message: format!("Parse error: {}", detail.into()),
            data: None,
        }
    }

    /// The JSON was valid but not a valid JSON-RPC request object (`-32600`).
    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self {
            code: Self::INVALID_REQUEST,
            message: format!("Invalid request: {}", detail.into()),
            data: None,
        }
    }

    pub fn method_not_found(method: &str) -> Self {
        Self {
            code: Self::METHOD_NOT_FOUND,
            message: format!("Method not found: {method}"),
            data: None,
        }
    }

    pub fn invalid_params(detail: impl Into<String>) -> Self {
        Self {
            code: Self::INVALID_PARAMS,
            message: format!("Invalid params: {}", detail.into()),
            data: None,
        }
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self {
            code: Self::INTERNAL,
            message: detail.into(),
            data: None,
        }
    }

    /// Map an `AppError` onto the right JSON-RPC error class for the built-in
    /// MCP servers (files / memory / skill / workflow), so client-class errors
    /// surface as method-not-found / invalid-params rather than a generic
    /// internal error. Shared so the built-in handlers can't drift.
    pub fn from_app_error(e: &ziee_core::AppError) -> Self {
        match e.status_code() {
            400 if e.error_code() == "UNKNOWN_TOOL" => {
                Self::method_not_found(&e.to_string())
            }
            // 4xx are client-class (bad input / access-denied / not-found /
            // stale) — surface as invalid_params so the LLM sees a client
            // error, not a server crash. skill_mcp / workflow_mcp return 403
            // (hidden / inaccessible / not-owner) and 410 (stale elicit),
            // which the older 400|404-only arm misclassified as internal.
            400 | 403 | 404 | 409 | 410 | 422 => Self::invalid_params(e.to_string()),
            _ => Self::internal(e.to_string()),
        }
    }
}

/// Pin a built-in MCP server's self-dial to loopback. The built-in servers
/// register their `mcp_servers.url` as `http://<host>:<port>/api/<x>/mcp` and
/// the MCP client dials it carrying the caller's JWT-signed bearer; this fn
/// derives the host portion of that self-dial and **always returns a loopback
/// address** — never the operator's `server.host` config value.
///
/// SECURITY: an earlier implementation passed `server.host` through
/// unchanged when it was a concrete address. That meant a config-set
/// `server.host = attacker.com` would register the built-in MCP
/// server's URL as `http://attacker.com:port/api/code-sandbox`, and
/// the MCP client (`mcp/client/manager.rs:78-113`) would then ship
/// every JWT-signed bearer + per-call context to attacker.com. This
/// matters because config / env-var (e.g. `SERVER__HOST=...`) is
/// often less guarded than DB credentials in container orchestration.
///
/// We pin to `127.0.0.1` because the loopback endpoint is the only
/// place this server can route the call to (we're invoking ourselves
/// through the local axum stack). The operator's `server.host` value
/// controls what the server BINDS to externally — but a sandbox
/// "loopback" must, by definition, dial `127.0.0.1`.
pub fn loopback_host(_server_host: &str) -> &str {
    "127.0.0.1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jsonrpc_request_round_trip() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).expect("parse");
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn jsonrpc_request_accepts_missing_jsonrpc_field() {
        let raw = r#"{"id":1,"method":"initialize"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).expect("parse");
        assert_eq!(req.jsonrpc, "2.0"); // default applied
    }

    #[test]
    fn jsonrpc_request_accepts_string_id() {
        let raw = r#"{"jsonrpc":"2.0","id":"abc","method":"x"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).expect("parse");
        assert_eq!(req.id, Some(serde_json::json!("abc")));
    }

    #[test]
    fn jsonrpc_error_helpers_have_canonical_codes() {
        let mnf = JsonRpcError::method_not_found("foo");
        assert_eq!(mnf.code, JsonRpcError::METHOD_NOT_FOUND);
        assert_eq!(mnf.code, -32601);

        let ip = JsonRpcError::invalid_params("bad");
        assert_eq!(ip.code, JsonRpcError::INVALID_PARAMS);
        assert_eq!(ip.code, -32602);

        let internal = JsonRpcError::internal("boom");
        assert_eq!(internal.code, JsonRpcError::INTERNAL);
        assert_eq!(internal.code, -32603);
    }

    #[test]
    fn jsonrpc_response_serializes_with_either_result_or_error() {
        let ok = JsonRpcResponse {
            jsonrpc: "2.0",
            id: Some(serde_json::json!(7)),
            result: Some(serde_json::json!({"x": 1})),
            error: None,
        };
        let s = serde_json::to_string(&ok).unwrap();
        assert!(s.contains("\"result\""));
        assert!(!s.contains("\"error\""));

        let err = JsonRpcResponse {
            jsonrpc: "2.0",
            id: None,
            result: None,
            error: Some(JsonRpcError::method_not_found("nope")),
        };
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains("\"error\""));
        assert!(!s.contains("\"result\""));
    }

    #[test]
    fn loopback_host_always_127_0_0_1_for_wildcards() {
        assert_eq!(loopback_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(loopback_host("::"), "127.0.0.1");
        assert_eq!(loopback_host("[::]"), "127.0.0.1");
        assert_eq!(loopback_host("0:0:0:0:0:0:0:0"), "127.0.0.1");
        assert_eq!(loopback_host(""), "127.0.0.1");
        assert_eq!(loopback_host("  "), "127.0.0.1");
    }

    #[test]
    fn loopback_host_pins_to_loopback_regardless_of_server_host() {
        // SECURITY regression test: the built-in MCP server's URL
        // must NEVER be configurable to a non-loopback address. If
        // server.host was `attacker.com`, an earlier implementation
        // would have passed that through and the MCP client would
        // ship JWT-signed bearer tokens to attacker.com per call.
        assert_eq!(loopback_host("attacker.com"), "127.0.0.1");
        assert_eq!(loopback_host("10.0.0.5"), "127.0.0.1");
        assert_eq!(loopback_host("169.254.169.254"), "127.0.0.1"); // IMDS
        assert_eq!(loopback_host("example.local"), "127.0.0.1");
        assert_eq!(loopback_host("[2001:db8::1]"), "127.0.0.1");
        // Even passing 127.0.0.1 itself yields the canonical form.
        assert_eq!(loopback_host("127.0.0.1"), "127.0.0.1");
    }
}
