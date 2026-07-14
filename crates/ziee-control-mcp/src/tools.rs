//! Static tool descriptors emitted by `tools/list` for the control MCP server.

use serde_json::{Value, json};

pub const LIST_CAPABILITIES: &str = "list_capabilities";
pub const DESCRIBE_CAPABILITY: &str = "describe_capability";
pub const INVOKE_CAPABILITY: &str = "invoke_capability";

pub fn tool_list() -> Value {
    json!({
        "tools": [
            {
                "name": LIST_CAPABILITIES,
                "description": "Discover what you can do to operate this ziee application on the user's behalf (create assistants, manage users, change settings, etc.). Returns a list of operations — each with its operation_id, HTTP method, and a one-line summary — filtered to what the current user is permitted to run (where a required permission is declared). Every operation is re-authorized when actually run, so anything the user isn't allowed to do is safely rejected. Use this to find the right operation_id, then `describe_capability` to learn its inputs, then `invoke_capability` to run it.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "description": "Optional free-text filter matched against operation_id, summary, and tags (e.g. \"assistant\", \"user\", \"web search\")."
                        },
                        "tag": {
                            "type": "string",
                            "description": "Optional exact tag filter (e.g. \"Users\", \"Assistants\")."
                        }
                    }
                }
            },
            {
                "name": DESCRIBE_CAPABILITY,
                "description": "Get the full input contract for one operation: its path parameters, query parameters, and request-body JSON Schema, plus its required permission. Call this before `invoke_capability` so you send correctly-shaped input. Returns a not-permitted error if the operation is known to require a permission the current user lacks.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "operation_id": {
                            "type": "string",
                            "description": "The operation_id from `list_capabilities` (e.g. \"Assistant.create\")."
                        }
                    },
                    "required": ["operation_id"]
                }
            },
            {
                "name": INVOKE_CAPABILITY,
                "description": "Run one operation against this ziee instance, exactly as if the user performed it in the UI. State-changing operations (create/update/delete) always require the user's explicit approval before they run. Provide path_params for any {…} placeholders, optional query parameters, and a body matching the operation's request schema. Returns the operation's real response (or its structured error, which you can use to correct and retry).",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "operation_id": {
                            "type": "string",
                            "description": "The operation_id to run (e.g. \"Assistant.create\")."
                        },
                        "path_params": {
                            "type": "object",
                            "description": "Values for the operation's {…} path parameters, keyed by name (e.g. {\"assistant_id\": \"…\"}).",
                            "additionalProperties": { "type": "string" }
                        },
                        "query": {
                            "type": "object",
                            "description": "Optional query-string parameters, keyed by name.",
                            "additionalProperties": true
                        },
                        "body": {
                            "type": "object",
                            "description": "The JSON request body, matching the operation's request schema (omit for operations that take no body).",
                            "additionalProperties": true
                        }
                    },
                    "required": ["operation_id"]
                }
            }
        ]
    })
}
