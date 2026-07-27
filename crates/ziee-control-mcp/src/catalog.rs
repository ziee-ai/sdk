//! In-process operation catalog for the built-in control MCP server.
//!
//! The catalog is the source of PRECISION: it is built ONCE at server start from
//! the fully-populated `aide::openapi::OpenApi` document (after `finish_api`),
//! so every operation the model can drive is exactly a real, registered REST
//! route with its real request schema + required permission. There is no
//! dependency on the on-disk `openapi.json` artifact (which can go stale).
//!
//! Populated by `init_from_openapi` at the two runtime bootstrap sites
//! (`main.rs` standalone + `lib.rs` embedded/desktop), immediately after
//! `finish_api(&mut api_doc)`. Read by `handlers.rs` for the three control tools.

use std::collections::HashMap;
use std::sync::OnceLock;

use serde_json::Value;

/// One drivable REST operation, distilled from the OpenAPI document.
#[derive(Debug, Clone)]
pub struct Operation {
    /// e.g. `User.create` — the stable key the model addresses.
    pub operation_id: String,
    /// Upper-case HTTP method (`GET`/`POST`/`PUT`/`DELETE`/`PATCH`).
    pub method: String,
    /// Path template as it appears in the spec, e.g. `/api/users/{user_id}`.
    pub path_template: String,
    pub tags: Vec<String>,
    pub summary: String,
    /// Parsed from the handler `_docs` description
    /// (`**Required Permission:** \`users::create\``). `None` when the route
    /// declares no permission (rare — e.g. health).
    pub required_permission: Option<String>,
    /// Names of `{...}` path parameters, in template order.
    pub path_params: Vec<String>,
    /// The request body's `application/json` schema (may contain `$ref`s into
    /// the shared components). `None` when the operation takes no JSON body.
    pub request_schema: Option<Value>,
    /// True when the operation has NO request body OR its body is
    /// `application/json`. False for multipart/octet-stream bodies (file
    /// uploads) — the control surface only drives JSON operations.
    pub json_body: bool,
    /// True when the request body has a top-level secret-bearing field
    /// (api_key / password / client_secret / token / …). Such ops are denied:
    /// driving them would persist the PLAINTEXT secret into the conversation's
    /// tool-call arguments. General rule that catches provider-key / auth-secret
    /// / password writes without enumerating paths.
    pub has_secret_field: bool,
    /// Resolved `parameters` array (query + path), for `describe_capability`.
    pub parameters: Vec<Value>,
}

/// The immutable, process-wide catalog.
#[derive(Debug)]
pub struct ControlCatalog {
    ops: HashMap<String, Operation>,
    /// The spec's `components` object, retained so request schemas that use
    /// `$ref: #/components/schemas/...` can be resolved for validation.
    components: Value,
}

impl ControlCatalog {
    pub fn get(&self, operation_id: &str) -> Option<&Operation> {
        self.ops.get(operation_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Operation> {
        self.ops.values()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    // Paired with `len` to satisfy clippy's `len_without_is_empty`; no caller yet.
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The shared `components` object (for `$ref` resolution during validation).
    pub fn components(&self) -> &Value {
        &self.components
    }
}

static CONTROL_CATALOG: OnceLock<ControlCatalog> = OnceLock::new();

/// Build + install the catalog from the finished OpenAPI document. Idempotent:
/// the first caller wins (subsequent calls are ignored), so the two bootstrap
/// sites and any test harness can all call it without ordering worries.
pub fn init_from_openapi(doc: &aide::openapi::OpenApi) {
    if CONTROL_CATALOG.get().is_some() {
        return;
    }
    let value = match serde_json::to_value(doc) {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("control_mcp: failed to serialize OpenAPI doc: {e}");
            return;
        }
    };
    let catalog = build_catalog(&value);
    let n = catalog.len();
    if CONTROL_CATALOG.set(catalog).is_ok() {
        tracing::info!("control_mcp: catalog initialized with {n} operations");
    }
}

/// The installed catalog, if `init_from_openapi` has run. `handlers.rs` returns
/// a clear "catalog unavailable" error when this is `None` (e.g. a desktop
/// bootstrap path that forgot to call `init_from_openapi`).
pub fn catalog() -> Option<&'static ControlCatalog> {
    CONTROL_CATALOG.get()
}

/// Pure builder (separated from the `OnceLock` install so it is unit-testable
/// against a fixture spec).
pub fn build_catalog(spec: &Value) -> ControlCatalog {
    let components = spec
        .get("components")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));
    let mut ops = HashMap::new();

    let Some(paths) = spec.get("paths").and_then(|p| p.as_object()) else {
        return ControlCatalog { ops, components };
    };

    for (path_template, item) in paths {
        let Some(item_obj) = item.as_object() else {
            continue;
        };
        // Path-level parameters apply to every method on the path.
        let path_level_params = item_obj
            .get("parameters")
            .and_then(|p| p.as_array())
            .cloned()
            .unwrap_or_default();

        for (method, op) in item_obj {
            if !is_http_method(method) {
                continue;
            }
            let Some(op_obj) = op.as_object() else {
                continue;
            };
            let Some(operation_id) = op_obj.get("operationId").and_then(|v| v.as_str()) else {
                continue;
            };

            let summary = op_obj
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let description = op_obj.get("description").and_then(|v| v.as_str());
            // The description is the PRIMARY source, but it is not a reliable
            // one: `with_permission` writes the permission into the operation's
            // description, and any handler `_docs` builder that then calls
            // `.description("…")` REPLACES that whole string. On the shipped
            // spec that silently loses the permission on 201 of the 408
            // operations that declare one — including `Project.create`, which
            // reported `required_permission: null` to the model. So fall back to
            // the 403 example `with_permission` also attaches, which no
            // `.description()` can clobber.
            let required_permission = description
                .and_then(parse_required_permission)
                .or_else(|| permission_from_403_example(op_obj));
            let tags = op_obj
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();

            let mut parameters = path_level_params.clone();
            if let Some(op_params) = op_obj.get("parameters").and_then(|p| p.as_array()) {
                parameters.extend(op_params.iter().cloned());
            }

            let (request_schema, json_body) = extract_request_body(op_obj);
            let has_secret_field = request_schema
                .as_ref()
                .map(|s| schema_has_secret_field(s, &components))
                .unwrap_or(false);

            let op_struct = Operation {
                operation_id: operation_id.to_string(),
                method: method.to_uppercase(),
                path_template: path_template.clone(),
                tags,
                summary,
                required_permission,
                path_params: extract_path_params(path_template),
                request_schema,
                json_body,
                has_secret_field,
                parameters,
            };
            ops.insert(operation_id.to_string(), op_struct);
        }
    }

    ControlCatalog { ops, components }
}

fn is_http_method(m: &str) -> bool {
    matches!(
        m,
        "get" | "post" | "put" | "delete" | "patch" | "head" | "options" | "trace"
    )
}

/// Extract the `application/json` request schema and whether the body is
/// JSON-shaped. Returns `(None, true)` for operations with no body (nothing to
/// validate, JSON-compatible), `(Some(schema), true)` for a JSON body, and
/// `(None, false)` for a non-JSON body (multipart/octet-stream uploads).
fn extract_request_body(op_obj: &serde_json::Map<String, Value>) -> (Option<Value>, bool) {
    let Some(content) = op_obj
        .get("requestBody")
        .and_then(|b| b.get("content"))
        .and_then(|c| c.as_object())
    else {
        return (None, true);
    };
    if let Some(json) = content.get("application/json") {
        let schema = json.get("schema").cloned();
        (schema, true)
    } else {
        // A body exists but is not JSON (multipart form upload, octet-stream).
        (None, false)
    }
}

/// True when the request schema (recursively — nested objects + array items,
/// resolving `$ref`s) has a secret-bearing property. Name-based; catches secrets
/// nested one or more levels deep (e.g. an MCP server's
/// `headers_entries[].is_secret`, an auth provider's `config.client_secret`).
fn schema_has_secret_field(schema: &Value, components: &Value) -> bool {
    schema_has_secret_field_rec(schema, components, 0)
}

fn schema_has_secret_field_rec(schema: &Value, components: &Value, depth: usize) -> bool {
    if depth > 6 {
        return false; // guard against pathological/cyclic schemas
    }
    let resolved = resolve_schema_ref(schema, components);
    if let Some(props) = resolved.get("properties").and_then(|p| p.as_object()) {
        for (name, subschema) in props {
            if is_secret_field_name(name) {
                return true;
            }
            if schema_has_secret_field_rec(subschema, components, depth + 1) {
                return true;
            }
        }
    }
    // Array element schemas.
    // (let-chain lowered to nested `if let` for the SDK workspace's edition 2021
    // — semantically identical to ziee's edition-2024 original.)
    if let Some(items) = resolved.get("items") {
        if schema_has_secret_field_rec(items, components, depth + 1) {
            return true;
        }
    }
    // Composition keywords (`anyOf`/`oneOf`/`allOf`) — e.g. a nullable
    // `auth_config` modeled as `anyOf: [$ref AuthConfig, null]`.
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = resolved.get(key).and_then(|v| v.as_array()) {
            if variants
                .iter()
                .any(|v| schema_has_secret_field_rec(v, components, depth + 1))
            {
                return true;
            }
        }
    }
    false
}

/// Word-boundary-aware secret-name test — avoids false positives like
/// `summarize_after_tokens` (a numeric count) matching bare `token`. A name is
/// secret when it contains an unambiguous compound (`api_key`, `client_secret`,
/// `private_key`, …) OR any `_`/`-`-delimited PART is a secret word.
fn is_secret_field_name(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    const COMPOUNDS: &[&str] = &["api_key", "apikey", "api-key", "private_key", "client_secret"];
    if COMPOUNDS.iter().any(|c| n.contains(c)) {
        return true;
    }
    // Token SECRETS only: the field named exactly `token`, or an `*_token` /
    // `*-token` suffix (access_token, api_token, download_token). Deliberately
    // NOT `token_*` (token_source), `*_tokens` (max_tokens), or `token_count`,
    // which are counts / selectors, not secrets.
    if n == "token" || n.ends_with("_token") || n.ends_with("-token") {
        return true;
    }
    // Unambiguous secret words as `_`/`-`-delimited parts.
    const SECRET_WORDS: &[&str] =
        &["password", "passwd", "secret", "credential", "credentials"];
    n.split(['_', '-']).any(|part| SECRET_WORDS.contains(&part))
}

/// Follow a single top-level `$ref: #/components/schemas/Name` into the shared
/// components; returns the input unchanged when it is not a `$ref`.
fn resolve_schema_ref(schema: &Value, components: &Value) -> Value {
    let Some(reference) = schema.get("$ref").and_then(|r| r.as_str()) else {
        return schema.clone();
    };
    let Some(name) = reference.strip_prefix("#/components/schemas/") else {
        return schema.clone();
    };
    components
        .get("schemas")
        .and_then(|s| s.get(name))
        .cloned()
        .unwrap_or_else(|| schema.clone())
}

/// `/api/users/{user_id}/reset-password` → `["user_id"]`.
pub fn extract_path_params(template: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        if let Some(close) = rest[open..].find('}') {
            let name = &rest[open + 1..open + close];
            if !name.is_empty() {
                out.push(name.to_string());
            }
            rest = &rest[open + close + 1..];
        } else {
            break;
        }
    }
    out
}

/// Parse the required permission out of the generated description line
/// `**Required Permission:** \`users::create\``. Returns the permission string
/// (`users::create`) or `None` when the pattern is absent.
pub fn parse_required_permission(description: &str) -> Option<String> {
    let marker = "Required Permission:";
    let idx = description.find(marker)?;
    let after = &description[idx + marker.len()..];
    // The permission is the first backtick-delimited token after the marker.
    let start = after.find('`')?;
    let rest = &after[start + 1..];
    let end = rest.find('`')?;
    let perm = rest[..end].trim();
    if perm.is_empty() {
        None
    } else {
        Some(perm.to_string())
    }
}

/// Recover the required permission from the documented 403 response.
///
/// `with_permission` attaches, alongside the description, a 403 example whose
/// `details.required_permissions[]` carries `{name, value, description}` per
/// permission. That example is structured data on a DIFFERENT part of the
/// operation, so a hand-written `.description("…")` cannot overwrite it — which
/// makes it the reliable source when the description has been replaced.
///
/// Takes the FIRST permission, which is exact parity with
/// [`parse_required_permission`] (it likewise returns only the first
/// backtick-delimited token). The 5 multi-permission operations are therefore
/// gated on their first permission rather than all of them — strictly more
/// correct than today, where the multi-permission heading
/// (`**Required Permissions (ALL):**`) does not even match the parser's marker
/// and they resolve to `None`.
fn permission_from_403_example(op_obj: &serde_json::Map<String, Value>) -> Option<String> {
    let perm = op_obj
        .get("responses")?
        .get("403")?
        .get("content")?
        .get("application/json")?
        .get("example")?
        .get("details")?
        .get("required_permissions")?
        .as_array()?
        .first()?
        .get("value")?
        .as_str()?
        .trim();
    if perm.is_empty() { None } else { Some(perm.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture_spec() -> Value {
        json!({
            "components": {
                "schemas": {
                    "UserCreate": {
                        "type": "object",
                        "required": ["username"],
                        "additionalProperties": false,
                        "properties": {
                            "username": { "type": "string" },
                            "email": { "type": "string" }
                        }
                    }
                }
            },
            "paths": {
                "/api/users": {
                    "post": {
                        "operationId": "User.create",
                        "summary": "Create a new user account",
                        "description": "\n\n**Required Permission:** `users::create`\n\nCreate new user accounts",
                        "tags": ["Users"],
                        "requestBody": {
                            "content": {
                                "application/json": {
                                    "schema": { "$ref": "#/components/schemas/UserCreate" }
                                }
                            }
                        }
                    },
                    "get": {
                        "operationId": "User.list",
                        "summary": "List users",
                        "description": "**Required Permission:** `users::read`",
                        "tags": ["Users"]
                    }
                },
                "/api/users/{user_id}": {
                    "delete": {
                        "operationId": "User.delete",
                        "summary": "Delete a user",
                        "description": "**Required Permission:** `users::delete`",
                        "tags": ["Users"]
                    }
                },
                "/api/files": {
                    "post": {
                        "operationId": "File.upload",
                        "summary": "Upload a file",
                        "description": "**Required Permission:** `files::upload`",
                        "requestBody": {
                            "content": {
                                "multipart/form-data": {
                                    "schema": { "type": "object" }
                                }
                            }
                        }
                    }
                },
                "/api/health": {
                    "get": {
                        "operationId": "Health.check",
                        "summary": "Health check"
                    }
                }
            }
        })
    }

    #[test]
    fn parses_operations_by_id() {
        let cat = build_catalog(&fixture_spec());
        assert_eq!(cat.len(), 5);
        let create = cat.get("User.create").unwrap();
        assert_eq!(create.method, "POST");
        assert_eq!(create.path_template, "/api/users");
        assert_eq!(create.summary, "Create a new user account");
        assert_eq!(create.tags, vec!["Users".to_string()]);
    }

    #[test]
    fn extracts_required_permission() {
        let cat = build_catalog(&fixture_spec());
        assert_eq!(
            cat.get("User.create").unwrap().required_permission.as_deref(),
            Some("users::create")
        );
        assert_eq!(
            cat.get("User.list").unwrap().required_permission.as_deref(),
            Some("users::read")
        );
    }

    #[test]
    fn no_permission_in_description_is_none() {
        let cat = build_catalog(&fixture_spec());
        assert!(cat.get("Health.check").unwrap().required_permission.is_none());
    }

    #[test]
    fn json_body_flag_distinguishes_multipart() {
        let cat = build_catalog(&fixture_spec());
        assert!(cat.get("User.create").unwrap().json_body);
        assert!(cat.get("User.create").unwrap().request_schema.is_some());
        // Multipart upload → not a JSON body, no request schema.
        assert!(!cat.get("File.upload").unwrap().json_body);
        assert!(cat.get("File.upload").unwrap().request_schema.is_none());
        // No body at all → still JSON-compatible.
        assert!(cat.get("User.list").unwrap().json_body);
    }

    #[test]
    fn path_params_extracted() {
        assert_eq!(
            extract_path_params("/api/users/{user_id}/reset-password"),
            vec!["user_id".to_string()]
        );
        assert_eq!(
            extract_path_params("/api/projects/{project_id}/files/{file_id}"),
            vec!["project_id".to_string(), "file_id".to_string()]
        );
        assert!(extract_path_params("/api/users").is_empty());
        let cat = build_catalog(&fixture_spec());
        assert_eq!(
            cat.get("User.delete").unwrap().path_params,
            vec!["user_id".to_string()]
        );
    }

    #[test]
    fn detects_secret_request_field() {
        let components = json!({
            "schemas": {
                "CreateProvider": {
                    "type": "object",
                    "properties": { "name": {"type":"string"}, "api_key": {"type":"string"} }
                }
            }
        });
        // Direct + via $ref both detect the secret field.
        assert!(schema_has_secret_field(
            &json!({ "type":"object", "properties": { "client_secret": {"type":"string"} } }),
            &components
        ));
        assert!(schema_has_secret_field(
            &json!({ "$ref": "#/components/schemas/CreateProvider" }),
            &components
        ));
        assert!(!schema_has_secret_field(
            &json!({ "type":"object", "properties": { "name": {"type":"string"} } }),
            &components
        ));
        // NESTED secret (object property + array items) is detected.
        assert!(schema_has_secret_field(
            &json!({ "type":"object", "properties": {
                "config": { "type":"object", "properties": { "client_secret": {"type":"string"} } }
            } }),
            &components
        ));
        assert!(schema_has_secret_field(
            &json!({ "type":"object", "properties": {
                "headers_entries": { "type":"array", "items": {
                    "type":"object", "properties": { "value": {"type":"string"}, "is_secret": {"type":"boolean"} }
                } }
            } }),
            &components
        ));
        // Nullable `anyOf: [$ref, null]` wrapper (real LlmRepository shape).
        let comp2 = json!({ "schemas": {
            "AuthCfg": { "type":"object", "properties": { "api_key": {"type":"string"} } }
        }});
        assert!(schema_has_secret_field(
            &json!({ "type":"object", "properties": {
                "auth_config": { "anyOf": [ {"$ref":"#/components/schemas/AuthCfg"}, {"type":"null"} ] }
            } }),
            &comp2
        ));
        // Name matching.
        assert!(is_secret_field_name("password"));
        assert!(is_secret_field_name("client_secret"));
        assert!(is_secret_field_name("API_KEY"));
        assert!(is_secret_field_name("access_token"));
        assert!(!is_secret_field_name("name"));
        assert!(is_secret_field_name("download_token"));
        // FALSE-POSITIVE guards: counts / selectors must NOT be treated as secret.
        assert!(!is_secret_field_name("summarize_after_tokens"));
        assert!(!is_secret_field_name("summarizer_keep_recent_tokens"));
        assert!(!is_secret_field_name("max_tokens"));
        assert!(!is_secret_field_name("token_source")); // mistralrs enum, not a secret
        assert!(!is_secret_field_name("token_count"));
    }

    #[test]
    fn iter_yields_all_ops_and_components_roundtrip() {
        let cat = build_catalog(&fixture_spec());
        // `iter()` visits every parsed op exactly once.
        let mut ids: Vec<String> = cat.iter().map(|o| o.operation_id.clone()).collect();
        ids.sort();
        assert_eq!(
            ids,
            vec![
                "File.upload",
                "Health.check",
                "User.create",
                "User.delete",
                "User.list",
            ]
        );
        assert!(!cat.is_empty());
        // `components()` retains the shared schemas for later $ref resolution.
        assert!(cat.components()["schemas"].get("UserCreate").is_some());
    }

    #[test]
    fn empty_spec_yields_empty_catalog() {
        // No `paths` key → an empty (is_empty) catalog with a defaulted
        // components object, not a panic.
        let cat = build_catalog(&json!({}));
        assert_eq!(cat.len(), 0);
        assert!(cat.is_empty());
        assert!(cat.components().is_object());
    }

    /// The shape a route produces when `with_permission` is followed by a
    /// hand-written `.description("…")`: the permission is GONE from the prose
    /// but still present, structured, in the documented 403. Modeled on the real
    /// `Project.create` entry in `src-app/ui/openapi/openapi.json`.
    fn clobbered_description_spec() -> Value {
        json!({
            "paths": {
                "/api/projects": {
                    "post": {
                        "operationId": "Project.create",
                        "summary": "Create a project",
                        // NOTE: no `**Required Permission:**` marker — the
                        // handler's own description replaced it.
                        "description": "Create a personal chat project.\n\nError codes:\n- `VALIDATION_ERROR` (400) — name empty.",
                        "responses": {
                            "403": {
                                "description": "Forbidden - Missing required permission",
                                "content": { "application/json": { "example": {
                                    "error": "Missing required permission: projects::create",
                                    "error_code": "INSUFFICIENT_PERMISSIONS",
                                    "details": { "required_permissions": [
                                        { "name": "ProjectsCreate", "value": "projects::create", "description": "Create chat projects" }
                                    ]}
                                }}}
                            }
                        }
                    }
                },
                "/api/both": {
                    "post": {
                        "operationId": "Both.write",
                        // Marker PRESENT — it must win over the 403 example.
                        "description": "**Required Permission:** `from::description`",
                        "responses": { "403": { "content": { "application/json": { "example": {
                            "details": { "required_permissions": [ { "value": "from::example" } ] }
                        }}}}}
                    }
                },
                "/api/multi": {
                    "post": {
                        "operationId": "Multi.write",
                        // The multi-permission heading does NOT match the
                        // parser's `Required Permission:` marker, so this only
                        // resolves via the 403 example.
                        "description": "**Required Permissions (ALL):**\n- `a::read` - Read a\n- `b::write` - Write b\n",
                        "responses": { "403": { "content": { "application/json": { "example": {
                            "details": { "required_permissions": [
                                { "value": "a::read" }, { "value": "b::write" }
                            ]}
                        }}}}}
                    }
                },
                "/api/malformed": {
                    "post": {
                        "operationId": "Malformed.write",
                        "description": "no marker",
                        "responses": { "403": { "content": { "application/json": { "example": {
                            "details": { "required_permissions": [ { "name": "NoValueKey" } ] }
                        }}}}}
                    }
                },
                "/api/blank": {
                    "post": {
                        "operationId": "Blank.write",
                        "description": "no marker",
                        "responses": { "403": { "content": { "application/json": { "example": {
                            "details": { "required_permissions": [ { "value": "   " } ] }
                        }}}}}
                    }
                }
            }
        })
    }

    /// TEST-13 — the defect: a route whose description was overwritten still
    /// reports its real permission, recovered from the documented 403.
    #[test]
    fn permission_recovered_from_403_example_when_description_was_overwritten() {
        let cat = build_catalog(&clobbered_description_spec());
        assert_eq!(
            cat.get("Project.create").unwrap().required_permission.as_deref(),
            Some("projects::create"),
            "Project.create reported null to the model because its permission \
             lived only in a description a handler `.description()` replaced"
        );
    }

    /// TEST-14 — the description marker keeps precedence (no regression for the
    /// operations that still carry it), a multi-permission op resolves to its
    /// first permission, and malformed/blank examples degrade to `None` rather
    /// than panicking or emitting an empty permission.
    #[test]
    fn description_marker_wins_and_malformed_examples_degrade() {
        let cat = build_catalog(&clobbered_description_spec());
        assert_eq!(
            cat.get("Both.write").unwrap().required_permission.as_deref(),
            Some("from::description"),
            "the description marker is the primary source"
        );
        assert_eq!(
            cat.get("Multi.write").unwrap().required_permission.as_deref(),
            Some("a::read"),
            "a multi-permission op resolves to its FIRST permission (parity with \
             the single-permission parser), not to None"
        );
        assert!(cat.get("Malformed.write").unwrap().required_permission.is_none());
        assert!(cat.get("Blank.write").unwrap().required_permission.is_none());

        // And an operation with NEITHER source is still None (the original
        // fixture's health check has no description and no 403).
        let base = build_catalog(&fixture_spec());
        assert!(base.get("Health.check").unwrap().required_permission.is_none());
    }

    #[test]
    fn parse_permission_edge_cases() {
        assert_eq!(
            parse_required_permission("**Required Permission:** `a::b::c`").as_deref(),
            Some("a::b::c")
        );
        assert!(parse_required_permission("no marker here").is_none());
        assert!(parse_required_permission("Required Permission: (none)").is_none());
    }
}
