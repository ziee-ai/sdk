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
                "description": "Get the full input contract for one operation: its path parameters, query parameters, and request-body JSON Schema, plus its required permission. References are resolved so the schema is self-contained — read `schema_form` (`inline`, or `defs` when shared/recursive types live in a sibling `$defs`) and `schema_truncated` to know whether any type was omitted for size. Call this before `invoke_capability` so you send correctly-shaped input. Returns a not-permitted error if the operation is known to require a permission the current user lacks.\n\nIf a REQUIRED field's value is still unknown, collect it with `ask_user` (one property per field) rather than asking in chat text.",
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
                "description": "Run one operation against this ziee instance, exactly as if the user performed it in the UI. State-changing operations (create/update/delete) always require the user's explicit approval before they run. Provide path_params for any {…} placeholders, optional query parameters, and a body matching the operation's request schema. Returns the operation's real response (or its structured error, which you can use to correct and retry).\n\nNever guess a required value: collect it with `ask_user` first, then invoke with the answers.",
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

#[cfg(test)]
mod tests {
    use super::{
        tool_list, DESCRIBE_CAPABILITY, INVOKE_CAPABILITY, LIST_CAPABILITIES,
    };

    /// `tools/list` is the MCP contract advertised to the model: exactly the
    /// three control tools, named by their public consts, each with an object
    /// `inputSchema`.
    #[test]
    fn tool_list_advertises_the_three_control_tools() {
        let v = tool_list();
        let tools = v["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), 3);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![LIST_CAPABILITIES, DESCRIBE_CAPABILITY, INVOKE_CAPABILITY]
        );
        for t in tools {
            assert_eq!(t["inputSchema"]["type"], "object", "tool {:?}", t["name"]);
            assert!(t["description"].as_str().is_some_and(|d| !d.is_empty()));
        }
    }

    /// `describe_capability` + `invoke_capability` REQUIRE `operation_id`;
    /// `list_capabilities` requires nothing (its query/tag filters are optional).
    #[test]
    fn required_inputs_match_each_tool() {
        let v = tool_list();
        let tools = v["tools"].as_array().unwrap();
        let by_name = |name: &str| {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap()
                .clone()
        };

        let list = by_name(LIST_CAPABILITIES);
        assert!(list["inputSchema"].get("required").is_none());
        assert!(list["inputSchema"]["properties"].get("query").is_some());
        assert!(list["inputSchema"]["properties"].get("tag").is_some());

        for name in [DESCRIBE_CAPABILITY, INVOKE_CAPABILITY] {
            let t = by_name(name);
            let req = t["inputSchema"]["required"].as_array().unwrap();
            assert!(
                req.iter().any(|r| r == "operation_id"),
                "{name} must require operation_id"
            );
        }

        // invoke exposes path_params / query / body inputs.
        let invoke = by_name(INVOKE_CAPABILITY);
        let props = &invoke["inputSchema"]["properties"];
        assert!(props.get("path_params").is_some());
        assert!(props.get("query").is_some());
        assert!(props.get("body").is_some());
    }

    /// TEST-17 — the model was writing "1. What's the project name? …" into the
    /// chat instead of using the built-in form tool. Both descriptors must carry
    /// the ask-with-a-form rule, and `describe_capability` must name the schema
    /// keys that make the form good — including `default`, which the wizard
    /// honours but the `ask_user` descriptor never mentions, so without it here
    /// "pre-filled" is unactionable. Guards against a silent revert.
    #[test]
    fn descriptions_instruct_ask_user_instead_of_prose() {
        let v = tool_list();
        let tools = v["tools"].as_array().unwrap();
        let desc = |name: &str| -> String {
            tools
                .iter()
                .find(|t| t["name"] == name)
                .unwrap()["description"]
                .as_str()
                .unwrap()
                .to_string()
        };

        let describe = desc(DESCRIBE_CAPABILITY);
        for needle in ["ask_user", "chat text"] {
            assert!(
                describe.contains(needle),
                "describe_capability must mention `{needle}`: {describe}"
            );
        }
        // The schema is self-contained but NOT unconditionally fully expanded —
        // a cycle or the size budget moves types into `$defs`, and the hard cap
        // can elide one. Promising "every field is spelled out" would teach the
        // model to ignore the two fields that report exactly that.
        for needle in ["schema_form", "schema_truncated"] {
            assert!(
                describe.contains(needle),
                "describe_capability must point at `{needle}` rather than overclaim: {describe}"
            );
        }
        assert!(
            !describe.contains("fully resolved"),
            "describe_capability must not claim unconditional full resolution: {describe}"
        );

        let invoke = desc(INVOKE_CAPABILITY);
        assert!(
            invoke.contains("ask_user"),
            "invoke_capability must mention `ask_user`: {invoke}"
        );
    }
}
