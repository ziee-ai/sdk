//! Rust port of `src-app/ui/openapi/generate-endpoints.ts`.
//!
//! Turns the in-memory OpenAPI spec (the same `openapi.json` the old TS script
//! consumed) into `ui/src/api-client/types.ts`, so a single
//! `cargo run -- --generate-openapi` produces both artifacts and the Node/tsx
//! codegen step can be dropped.
//!
//! This is a FAITHFUL, byte-for-byte port: the `types_ts_parity` test asserts
//! the output is identical to the committed `types.ts` for the committed
//! `openapi.json`. Any behavioural change here must be mirrored intentionally.
//!
//! Parity notes:
//! - JSON object **insertion order** is significant (schema property order, SSE
//!   variant order). We therefore deserialize into [`J`], an `IndexMap`-backed
//!   value, NOT `serde_json::Value` (which would alphabetize via `BTreeMap`).
//! - JS `Array.prototype.sort()` over ASCII keys == byte order == Rust's default
//!   `slice::sort`. The one exception is the permissions list, which used
//!   `localeCompare`; for these PascalCase identifiers that is equivalent to a
//!   case-insensitive (`to_lowercase`) compare with the original string as
//!   tiebreak — replicated in [`generate_typescript_content`].

use indexmap::IndexMap;
use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;

// =============================================================================
// Order-preserving JSON value
// =============================================================================

#[derive(Clone, Debug)]
pub enum J {
    Null,
    Bool(bool),
    Num(serde_json::Number),
    Str(String),
    Arr(Vec<J>),
    Obj(IndexMap<String, J>),
}

impl<'de> Deserialize<'de> for J {
    fn deserialize<D>(deserializer: D) -> Result<J, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = J;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_unit<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_none<E>(self) -> Result<J, E> {
                Ok(J::Null)
            }
            fn visit_bool<E>(self, b: bool) -> Result<J, E> {
                Ok(J::Bool(b))
            }
            fn visit_i64<E>(self, n: i64) -> Result<J, E> {
                Ok(J::Num(n.into()))
            }
            fn visit_u64<E>(self, n: u64) -> Result<J, E> {
                Ok(J::Num(n.into()))
            }
            fn visit_f64<E>(self, n: f64) -> Result<J, E> {
                Ok(serde_json::Number::from_f64(n).map(J::Num).unwrap_or(J::Null))
            }
            fn visit_str<E>(self, s: &str) -> Result<J, E> {
                Ok(J::Str(s.to_string()))
            }
            fn visit_string<E>(self, s: String) -> Result<J, E> {
                Ok(J::Str(s))
            }
            fn visit_seq<A>(self, mut seq: A) -> Result<J, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut v = Vec::new();
                while let Some(e) = seq.next_element()? {
                    v.push(e);
                }
                Ok(J::Arr(v))
            }
            fn visit_map<A>(self, mut map: A) -> Result<J, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut m = IndexMap::new();
                while let Some((k, val)) = map.next_entry::<String, J>()? {
                    m.insert(k, val);
                }
                Ok(J::Obj(m))
            }
        }
        deserializer.deserialize_any(V)
    }
}

impl J {
    fn get(&self, key: &str) -> Option<&J> {
        match self {
            J::Obj(m) => m.get(key),
            _ => None,
        }
    }
    fn as_str(&self) -> Option<&str> {
        match self {
            J::Str(s) => Some(s),
            _ => None,
        }
    }
    fn as_array(&self) -> Option<&Vec<J>> {
        match self {
            J::Arr(a) => Some(a),
            _ => None,
        }
    }
    fn as_object(&self) -> Option<&IndexMap<String, J>> {
        match self {
            J::Obj(m) => Some(m),
            _ => None,
        }
    }
    /// Mirrors `isSchemaReference`: an object with a `$ref` key.
    fn is_ref(&self) -> bool {
        self.get("$ref").is_some()
    }
    /// `schema.type` when it is a single string.
    fn type_string(&self) -> Option<&str> {
        match self.get("type") {
            Some(J::Str(s)) => Some(s),
            _ => None,
        }
    }
    /// `schema.type` when it is an array of strings.
    fn type_array(&self) -> Option<Vec<&str>> {
        match self.get("type") {
            Some(J::Arr(a)) => Some(a.iter().filter_map(|x| x.as_str()).collect()),
            _ => None,
        }
    }
    fn has_key(&self, key: &str) -> bool {
        matches!(self, J::Obj(m) if m.contains_key(key))
    }
    /// `required?.includes(name)`
    fn required_includes(&self, name: &str) -> bool {
        match self.get("required") {
            Some(J::Arr(a)) => a.iter().any(|x| x.as_str() == Some(name)),
            _ => false,
        }
    }
}

/// Render a schemars `description` (from a Rust `///` doc-comment) as a JSDoc
/// block at the given indent. Returns `None` for an empty/whitespace-only
/// description. `*/` is escaped so a description can never close the comment
/// early. (Hardening over the original TS generator, which dropped every
/// doc-comment.)
fn render_doc(desc: &str, indent: &str) -> Option<String> {
    let desc = desc.trim_end_matches('\n');
    if desc.trim().is_empty() {
        return None;
    }
    let lines: Vec<String> = desc
        .split('\n')
        .map(|l| l.trim_end().replace("*/", "*\\/"))
        .collect();
    if lines.len() == 1 {
        Some(format!("{}/** {} */", indent, lines[0].trim()))
    } else {
        let mut out = format!("{}/**\n", indent);
        for l in &lines {
            if l.is_empty() {
                out.push_str(&format!("{} *\n", indent));
            } else {
                out.push_str(&format!("{} * {}\n", indent, l));
            }
        }
        out.push_str(&format!("{} */", indent));
        Some(out)
    }
}

/// Escape a string for placement inside a single-quoted TS string literal.
/// (Hardening beyond the original JS generator, which emitted raw `'{value}'`
/// — an unescaped `'` or `\` produced TypeScript that `tsc` rejects. A no-op
/// for the current spec, so the golden output is unchanged.)
fn ts_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "\\'")
}

/// A single-quoted, escaped TS string literal: `'value'`.
fn ts_str_lit(s: &str) -> String {
    format!("'{}'", ts_escape(s))
}

/// Whether `s` is a valid (ASCII) TS identifier usable as an UNQUOTED member
/// name. Reserved words are fine as member names, so they're not excluded.
fn is_ts_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c == '_' || c == '$' || c.is_ascii_alphabetic() => {}
        _ => return false,
    }
    chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric())
}

/// An object/interface member key: bare if a valid identifier, else a quoted
/// string literal. (The original JS emitted the key raw, breaking `tsc` on
/// e.g. a `#[serde(rename = "foo-bar")]`. No-op for the current spec.)
fn ts_prop_key(name: &str) -> String {
    if is_ts_identifier(name) {
        name.to_string()
    } else {
        ts_str_lit(name)
    }
}

/// JS `String(x)` for the non-string `const` branch (bool / number).
fn js_stringify_scalar(j: &J) -> String {
    match j {
        J::Bool(b) => b.to_string(),
        J::Num(n) => n.to_string(),
        J::Str(s) => s.clone(),
        J::Null => "null".to_string(),
        _ => "undefined".to_string(),
    }
}

// =============================================================================
// Permission info
// =============================================================================

struct PermissionInfo {
    name: String,
    value: String,
    description: String,
}

fn extract_schema_name(reference: &str) -> String {
    let schema_name = reference.replace("#/components/schemas/", "");
    if schema_name == "AnyType" {
        return "any".to_string();
    }
    if schema_name == "BlobType" {
        return "Blob".to_string();
    }
    schema_name
}

fn extract_permissions_from_spec(spec: &J) -> Vec<PermissionInfo> {
    // Keyed by name to dedup; order rebuilt by sort below.
    let mut map: IndexMap<String, PermissionInfo> = IndexMap::new();

    let paths = match spec.get("paths").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return Vec::new(),
    };

    for (_, methods) in paths {
        let methods = match methods.as_object() {
            Some(m) => m,
            None => continue,
        };
        for (_, operation) in methods {
            let forbidden = operation.get("responses").and_then(|r| r.get("403"));
            let content = match forbidden.and_then(|f| f.get("content")) {
                Some(c) => c,
                None => continue,
            };
            let json_content = match content.get("application/json") {
                Some(c) => c,
                None => continue,
            };
            let required_perms = json_content
                .get("example")
                .and_then(|e| e.get("details"))
                .and_then(|d| d.get("required_permissions"));
            if let Some(J::Arr(perms)) = required_perms {
                for perm in perms {
                    let name = perm.get("name").and_then(|x| x.as_str());
                    let value = perm.get("value").and_then(|x| x.as_str());
                    let description = perm.get("description").and_then(|x| x.as_str());
                    if let (Some(name), Some(value), Some(description)) = (name, value, description) {
                        map.insert(
                            name.to_string(),
                            PermissionInfo {
                                name: name.to_string(),
                                value: value.to_string(),
                                description: description.to_string(),
                            },
                        );
                    }
                }
            }
        }
    }

    let mut out: Vec<PermissionInfo> = map.into_values().collect();
    // JS `a.name.localeCompare(b.name)` == case-insensitive then byte tiebreak
    // for these PascalCase identifiers.
    out.sort_by(|a, b| {
        let ka = a.name.to_lowercase();
        let kb = b.name.to_lowercase();
        ka.cmp(&kb).then_with(|| a.name.cmp(&b.name))
    });
    out
}

fn detect_query_schema_type(query_params: &[&J]) -> Option<String> {
    if query_params.is_empty() {
        return None;
    }
    let mut names: Vec<&str> = query_params
        .iter()
        .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
        .collect();
    names.sort();
    if names.len() == 2 && names.contains(&"page") && names.contains(&"per_page") {
        return Some("PaginationQuery".to_string());
    }
    None
}

/// Extract `{param}` segments from a path, in order.
fn path_params(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(end) = path[i + 1..].find('}') {
                out.push(path[i + 1..i + 1 + end].to_string());
                i = i + 1 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn generate_parameter_type(operation: &J, path: &str) -> String {
    let mut param_types: Vec<String> = Vec::new();

    for p in path_params(path) {
        param_types.push(format!("{}: string", ts_prop_key(&p)));
    }

    let mut query_params: Vec<String> = Vec::new();
    let mut query_schema_type: Option<String> = None;

    let params_array: Vec<&J> = operation
        .get("parameters")
        .and_then(|p| p.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();

    if operation.get("parameters").is_some() {
        for param in &params_array {
            if param.get("in").and_then(|x| x.as_str()) == Some("query") {
                let is_optional = !matches!(param.get("required"), Some(J::Bool(true)));
                let schema = param.get("schema").cloned().unwrap_or(J::Null);
                let param_type = get_type_from_schema(&schema, is_optional);
                let name = param.get("name").and_then(|x| x.as_str()).unwrap_or("");
                query_params.push(format!(
                    "{}{}: {}",
                    ts_prop_key(name),
                    if is_optional { "?" } else { "" },
                    param_type
                ));
            }
        }
        let query_only: Vec<&J> = params_array
            .iter()
            .filter(|p| p.get("in").and_then(|x| x.as_str()) == Some("query"))
            .copied()
            .collect();
        query_schema_type = detect_query_schema_type(&query_only);
    }

    if query_schema_type.is_some() {
        // Don't add individual query params; use the schema type.
    } else {
        param_types.extend(query_params.iter().cloned());
    }

    // Request body type
    let mut request_body_type: Option<String> = None;
    if let Some(request_body) = operation.get("requestBody") {
        let content_obj = request_body.get("content");
        let mut content = content_obj.and_then(|c| c.get("application/json"));
        if content.is_none() {
            content = content_obj.and_then(|c| c.get("multipart/form-data"));
        }
        if let Some(content) = content {
            let schema = content.get("schema");
            if let Some(schema) = schema {
                if schema.is_ref() {
                    request_body_type = Some(extract_schema_name(
                        schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""),
                    ));
                } else if content_obj
                    .and_then(|c| c.get("multipart/form-data"))
                    .is_some()
                {
                    request_body_type = Some("FormData".to_string());
                } else {
                    request_body_type = Some("any".to_string());
                }
            } else if content_obj
                .and_then(|c| c.get("multipart/form-data"))
                .is_some()
            {
                request_body_type = Some("FormData".to_string());
            } else {
                request_body_type = Some("any".to_string());
            }
        }
    }

    let has_query_params = params_array
        .iter()
        .any(|p| p.get("in").and_then(|x| x.as_str()) == Some("query"));

    if param_types.is_empty() && query_schema_type.is_none() && request_body_type.is_none() {
        "void".to_string()
    } else if param_types.is_empty() && query_schema_type.is_none() && request_body_type.is_some() {
        request_body_type.unwrap()
    } else if query_schema_type.is_some() && request_body_type.is_none() && param_types.is_empty() {
        query_schema_type.unwrap()
    } else if query_schema_type.is_some() && request_body_type.is_some() && param_types.is_empty() {
        format!(
            "{} & {}",
            query_schema_type.unwrap(),
            request_body_type.unwrap()
        )
    } else if query_schema_type.is_some() && !param_types.is_empty() && request_body_type.is_none() {
        format!(
            "{{ {} }} & {}",
            param_types.join("; "),
            query_schema_type.unwrap()
        )
    } else if query_schema_type.is_some() && !param_types.is_empty() && request_body_type.is_some() {
        format!(
            "{{ {} }} & {} & {}",
            param_types.join("; "),
            query_schema_type.unwrap(),
            request_body_type.unwrap()
        )
    } else if !param_types.is_empty() && request_body_type.is_some() {
        format!("{{ {} }} & {}", param_types.join("; "), request_body_type.unwrap())
    } else if param_types.len() == 1 && !has_query_params {
        format!("{{ {} }}", param_types[0])
    } else {
        format!("{{ {} }}", param_types.join("; "))
    }
}

fn generate_response_type(operation: &J, http_method: Option<&str>) -> String {
    let responses = operation.get("responses");
    if responses.is_none() || responses.and_then(|r| r.get("204")).is_some() {
        return "void".to_string();
    }
    let responses = responses.unwrap();

    let success = responses
        .get("200")
        .or_else(|| responses.get("201"))
        .or_else(|| responses.get("202"));

    let success = match success {
        Some(s) => s,
        None => return "any".to_string(),
    };

    let content = match success.get("content") {
        Some(c) => c,
        None => {
            return if http_method == Some("POST") {
                "any".to_string()
            } else {
                "void".to_string()
            }
        }
    };

    let json_content = content.get("application/json");
    let schema = json_content.and_then(|j| j.get("schema"));
    if json_content.is_none() || schema.is_none() {
        // Both branches in the TS return 'any' here.
        return "any".to_string();
    }
    let schema = schema.unwrap();

    if schema.is_ref() {
        extract_schema_name(schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""))
    } else {
        get_type_from_schema(schema, false)
    }
}

fn get_type_from_schema(schema: &J, is_optional_or_nullable: bool) -> String {
    if let J::Bool(b) = schema {
        // A `serde_json::Value` field renders as the boolean schema `true`
        // (accepts any JSON); `false` accepts nothing. Type these precisely
        // rather than collapsing to `any`. (`AnyType` keeps its explicit `any`
        // via extract_schema_name — that's a deliberate escape hatch.)
        return if *b {
            "unknown".to_string()
        } else {
            "never".to_string()
        };
    }

    if schema.is_ref() {
        let type_name =
            extract_schema_name(schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""));

        if let Some(actual) = type_name.strip_prefix("JsonOption_for_") {
            if actual == "Array_of_File" {
                return "File[]".to_string();
            } else if actual == "Array_of_MessageMetadata" {
                return "MessageMetadata[]".to_string();
            } else if actual == "Array_of_string" {
                return "string[]".to_string();
            } else if let Some(item) = actual.strip_prefix("Array_of_") {
                return format!("{}[]", item);
            } else {
                return actual.to_string();
            }
        }
        if let Some(actual) = type_name.strip_prefix("EnumOption_for_") {
            return actual.to_string();
        }
        return type_name;
    }

    // anyOf
    if let Some(J::Arr(any_of)) = schema.get("anyOf") {
        let types: Vec<String> = any_of
            .iter()
            .filter_map(|sub| {
                if sub.is_ref() {
                    Some(extract_schema_name(
                        sub.get("$ref").and_then(|x| x.as_str()).unwrap_or(""),
                    ))
                } else if sub.type_string() == Some("null") {
                    if is_optional_or_nullable {
                        None
                    } else {
                        Some("null".to_string())
                    }
                } else {
                    Some(get_type_from_schema(sub, is_optional_or_nullable))
                }
            })
            .collect();
        return if types.len() == 1 {
            types[0].clone()
        } else {
            types.join(" | ")
        };
    }

    // oneOf
    if let Some(J::Arr(one_of)) = schema.get("oneOf") {
        let types: Vec<String> = one_of
            .iter()
            .map(|sub| {
                if sub.is_ref() {
                    extract_schema_name(sub.get("$ref").and_then(|x| x.as_str()).unwrap_or(""))
                } else if sub.type_string() == Some("object") && sub.get("properties").is_some() {
                    let mut props: Vec<String> = Vec::new();
                    if let Some(properties) = sub.get("properties").and_then(|p| p.as_object()) {
                        for (prop_name, prop_schema) in properties {
                            let mut prop_type = get_type_from_schema(prop_schema, false);
                            if prop_schema.has_key("const") {
                                let c = prop_schema.get("const").unwrap();
                                prop_type = match c {
                                    J::Str(s) => ts_str_lit(s),
                                    other => js_stringify_scalar(other),
                                };
                            }
                            let is_required = sub.required_includes(prop_name);
                            let marker = if is_required { "" } else { "?" };
                            props.push(format!("{}{}: {}", ts_prop_key(prop_name), marker, prop_type));
                        }
                    }
                    format!("{{ {} }}", props.join("; "))
                } else if sub.has_key("const") {
                    let c = sub.get("const").unwrap();
                    match c {
                        J::Str(s) => ts_str_lit(s),
                        other => js_stringify_scalar(other),
                    }
                } else {
                    get_type_from_schema(sub, is_optional_or_nullable)
                }
            })
            .collect();
        return if types.len() == 1 {
            types[0].clone()
        } else {
            types.join(" | ")
        };
    }

    // allOf
    if let Some(J::Arr(all_of)) = schema.get("allOf") {
        let types: Vec<String> = all_of
            .iter()
            .map(|sub| {
                if sub.is_ref() {
                    extract_schema_name(sub.get("$ref").and_then(|x| x.as_str()).unwrap_or(""))
                } else {
                    get_type_from_schema(sub, is_optional_or_nullable)
                }
            })
            .collect();
        if types.len() == 1 {
            return types[0].clone();
        }
        return types.join(" & ");
    }

    // enum
    if let Some(J::Arr(enum_values)) = schema.get("enum") {
        return enum_values
            .iter()
            .map(|v| ts_str_lit(&js_stringify_scalar(v)))
            .collect::<Vec<_>>()
            .join(" | ");
    }

    if let Some(t) = schema.type_string() {
        match t {
            "string" => "string".to_string(),
            "integer" | "number" => "number".to_string(),
            "boolean" => "boolean".to_string(),
            "array" => {
                if let Some(items) = schema.get("items") {
                    format!("{}[]", get_type_from_schema(items, false))
                } else {
                    "any[]".to_string()
                }
            }
            "object" => {
                if let Some(properties) = schema.get("properties").and_then(|p| p.as_object()) {
                    let mut props: Vec<String> = Vec::new();
                    for (prop_name, prop_schema) in properties {
                        let prop_type = get_type_from_schema(prop_schema, false);
                        props.push(format!("{}: {}", ts_prop_key(prop_name), prop_type));
                    }
                    format!("{{ {} }}", props.join("; "))
                } else {
                    "any".to_string()
                }
            }
            _ => "any".to_string(),
        }
    } else if let Some(t_arr) = schema.type_array() {
        let types: Vec<String> = t_arr
            .iter()
            .filter(|t| !(is_optional_or_nullable && **t == "null"))
            .map(|t| match *t {
                "string" => "string".to_string(),
                "integer" | "number" => "number".to_string(),
                "boolean" => "boolean".to_string(),
                "array" => {
                    if let Some(items) = schema.get("items") {
                        format!("{}[]", get_type_from_schema(items, false))
                    } else {
                        "any[]".to_string()
                    }
                }
                "null" => "null".to_string(),
                _ => "any".to_string(),
            })
            .collect();
        if types.len() == 1 {
            types[0].clone()
        } else {
            types.join(" | ")
        }
    } else {
        // No $ref / anyOf / oneOf / allOf / enum / type — i.e. a schema that
        // accepts any JSON (e.g. a `serde_json::Value` field carrying only a
        // description / writeOnly). Type it as `unknown`, not `any`.
        "unknown".to_string()
    }
}

fn generate_schema_interface(name: &str, schema: &J) -> String {
    let body = generate_schema_interface_body(name, schema);
    // Skipped types (JsonOption_for_ / EnumOption_for_) return empty — leave as-is.
    if body.trim().is_empty() {
        return body;
    }
    // Hardening: carry the type's own doc-comment through as a JSDoc block above
    // the declaration that NAMES this type. For multi-declaration bodies (only
    // `MessageContentData`, which emits per-variant interfaces followed by the
    // union) this places the doc on the `export type MessageContentData = …`
    // line, not the first variant interface.
    if let Some(desc) = schema.get("description").and_then(|d| d.as_str()) {
        if let Some(doc) = render_doc(desc, "") {
            let decl_prefixes = [
                format!("export interface {} ", name),
                format!("export type {} ", name),
                format!("export enum {} ", name),
            ];
            let lines: Vec<&str> = body.split('\n').collect();
            let idx = lines
                .iter()
                .position(|l| decl_prefixes.iter().any(|p| l.starts_with(p.as_str())));
            return match idx {
                Some(i) => {
                    let mut out: Vec<String> = lines[..i].iter().map(|s| s.to_string()).collect();
                    out.push(doc);
                    out.extend(lines[i..].iter().map(|s| s.to_string()));
                    out.join("\n")
                }
                // No declaration line found (shouldn't happen for a non-empty
                // body) — fall back to prepending.
                None => format!("{}\n{}", doc, body),
            };
        }
    }
    body
}

fn generate_schema_interface_body(name: &str, schema: &J) -> String {
    if schema.has_key("$ref") {
        return format!(
            "export type {} = {}",
            name,
            extract_schema_name(schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""))
        );
    }

    if name.starts_with("JsonOption_for_") || name.starts_with("EnumOption_for_") {
        return String::new();
    }

    if name.starts_with("SSE") {
        if let Some(J::Arr(one_of)) = schema.get("oneOf") {
            return generate_sse_event_type(name, one_of);
        }
    }

    if name == "Permission" {
        if let Some(J::Arr(enum_values)) = schema.get("enum") {
            return generate_permission_enum(enum_values);
        }
    }

    if let Some(J::Arr(one_of)) = schema.get("oneOf") {
        if name == "MessageContentData" {
            return generate_message_content_data_types(one_of);
        }

        let types: Vec<String> = one_of
            .iter()
            .map(|sub| {
                if sub.is_ref() {
                    extract_schema_name(sub.get("$ref").and_then(|x| x.as_str()).unwrap_or(""))
                } else if sub.type_string() == Some("object") && sub.get("properties").is_some() {
                    let mut props: Vec<String> = Vec::new();
                    if let Some(properties) = sub.get("properties").and_then(|p| p.as_object()) {
                        for (prop_name, prop_schema) in properties {
                            let mut prop_type = get_type_from_schema(prop_schema, false);
                            if prop_schema.has_key("const") {
                                let c = prop_schema.get("const").unwrap();
                                prop_type = match c {
                                    J::Str(s) => ts_str_lit(s),
                                    other => js_stringify_scalar(other),
                                };
                            }
                            let is_required = sub.required_includes(prop_name);
                            let marker = if is_required { "" } else { "?" };
                            props.push(format!("  {}{}: {}", ts_prop_key(prop_name), marker, prop_type));
                        }
                    }
                    format!("{{\n{}\n}}", props.join("\n"))
                } else if sub.has_key("const") {
                    let c = sub.get("const").unwrap();
                    match c {
                        J::Str(s) => ts_str_lit(s),
                        other => js_stringify_scalar(other),
                    }
                } else {
                    get_type_from_schema(sub, false)
                }
            })
            .collect();
        return format!("export type {} = {}", name, types.join(" | "));
    }

    if schema.type_string() == Some("object") && schema.get("properties").is_some() {
        let mut properties: Vec<String> = Vec::new();
        if let Some(props) = schema.get("properties").and_then(|p| p.as_object()) {
            for (prop_name, prop_schema) in props {
                let mut is_optional = !schema.required_includes(prop_name);

                let prop_type: String;
                if prop_schema.is_ref() {
                    let ref_type_name = extract_schema_name(
                        prop_schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""),
                    );
                    if ref_type_name.starts_with("JsonOption_for_")
                        || ref_type_name.starts_with("EnumOption_for_")
                    {
                        is_optional = true;
                        prop_type = get_type_from_schema(prop_schema, true);
                    } else {
                        prop_type = get_type_from_schema(prop_schema, false);
                    }
                } else {
                    let is_nullable_union = prop_schema
                        .type_array()
                        .map(|a| a.contains(&"null"))
                        .unwrap_or(false);
                    let is_nullable_any_of = matches!(prop_schema.get("anyOf"), Some(J::Arr(a))
                        if a.iter().any(|s| s.type_string() == Some("null")));
                    let is_nullable_all_of = matches!(prop_schema.get("allOf"), Some(J::Arr(a))
                        if a.iter().any(|s| s.type_string() == Some("null")));
                    let is_nullable = is_nullable_union || is_nullable_any_of || is_nullable_all_of;

                    if is_nullable {
                        is_optional = true;
                    }
                    prop_type = get_type_from_schema(prop_schema, is_nullable);
                }

                // Hardening: carry the field's doc-comment through as JSDoc.
                if let Some(desc) = prop_schema.get("description").and_then(|d| d.as_str()) {
                    if let Some(doc) = render_doc(desc, "  ") {
                        properties.push(doc);
                    }
                }

                let marker = if is_optional { "?" } else { "" };
                properties.push(format!("  {}{}: {}", ts_prop_key(prop_name), marker, prop_type));
            }
        }
        format!("export interface {} {{\n{}\n}}", name, properties.join("\n"))
    } else if schema.type_string() == Some("array") && schema.get("items").is_some() {
        let item_type = get_type_from_schema(schema.get("items").unwrap(), false);
        format!("export type {} = {}[]", name, item_type)
    } else {
        let base_type = get_type_from_schema(schema, false);
        format!("export type {} = {}", name, base_type)
    }
}

fn generate_sse_event_type(name: &str, one_of: &[J]) -> String {
    let mut event_types: Vec<String> = Vec::new();

    for variant in one_of {
        // Discriminated union pattern: properties.type.const + allOf[0] data.
        if variant.type_string() == Some("object") {
            if let Some(type_prop) = variant.get("properties").and_then(|p| p.get("type")) {
                if let Some(c) = type_prop.get("const") {
                    let event_name = js_stringify_scalar(c);
                    let mut data_type = "any".to_string();
                    if let Some(J::Arr(all_of)) = variant.get("allOf") {
                        if let Some(data_schema) = all_of.first() {
                            if data_schema.is_ref() {
                                data_type = extract_schema_name(
                                    data_schema.get("$ref").and_then(|x| x.as_str()).unwrap_or(""),
                                );
                            } else {
                                data_type = get_type_from_schema(data_schema, false);
                            }
                        }
                    }
                    event_types.push(format!("  {}: {}", ts_prop_key(&event_name), data_type));
                    continue;
                }
            }
        }

        // Simple object-based pattern: single property = event name -> data.
        if variant.type_string() == Some("object") {
            if let Some(properties) = variant.get("properties").and_then(|p| p.as_object()) {
                let keys: Vec<&String> = properties.keys().collect();
                if keys.len() == 1 {
                    let event_name = keys[0];
                    let event_data_schema = &properties[keys[0]];
                    let data_type = if event_data_schema.is_ref() {
                        extract_schema_name(
                            event_data_schema
                                .get("$ref")
                                .and_then(|x| x.as_str())
                                .unwrap_or(""),
                        )
                    } else {
                        get_type_from_schema(event_data_schema, false)
                    };
                    event_types.push(format!("  {}: {}", ts_prop_key(event_name), data_type));
                }
            }
        }
    }

    format!("export type {} = {{\n{}\n}}", name, event_types.join("\n"))
}

fn generate_message_content_data_types(one_of: &[J]) -> String {
    let mut type_definitions: Vec<String> = Vec::new();
    let mut union_types: Vec<String> = Vec::new();

    for variant in one_of {
        if variant.type_string() == Some("object") {
            if let Some(properties) = variant.get("properties").and_then(|p| p.as_object()) {
                let type_prop = properties.get("type");
                let type_const = type_prop.and_then(|t| t.get("const"));
                if let Some(type_const) = type_const {
                    let type_value = js_stringify_scalar(type_const);
                    let type_name = format!(
                        "MessageContentData{}",
                        pascal_from_snake_first_upper(&type_value)
                    );

                    let mut props: Vec<String> = Vec::new();
                    for (prop_name, prop_schema) in properties {
                        let mut prop_type = get_type_from_schema(prop_schema, false);
                        if prop_schema.has_key("const") {
                            let c = prop_schema.get("const").unwrap();
                            prop_type = match c {
                                J::Str(s) => ts_str_lit(s),
                                other => js_stringify_scalar(other),
                            };
                        }
                        // Hardening: carry variant field doc-comments through.
                        if let Some(desc) = prop_schema.get("description").and_then(|d| d.as_str()) {
                            if let Some(doc) = render_doc(desc, "  ") {
                                props.push(doc);
                            }
                        }
                        let is_required = variant.required_includes(prop_name);
                        let marker = if is_required { "" } else { "?" };
                        props.push(format!("  {}{}: {}", ts_prop_key(prop_name), marker, prop_type));
                    }

                    type_definitions.push(format!(
                        "export interface {} {{\n{}\n}}",
                        type_name,
                        props.join("\n")
                    ));
                    union_types.push(type_name);
                }
            }
        }
    }

    let main_union = format!(
        "export type MessageContentData = {}",
        union_types.join(" | ")
    );

    let mut parts = type_definitions;
    parts.push(String::new());
    parts.push(main_union);
    parts.join("\n")
}

/// Replicates: `typeValue.charAt(0).toUpperCase() + typeValue.slice(1)`
/// with `.replace(/_([a-z])/g, (_, l) => l.toUpperCase())` on the slice.
fn pascal_from_snake_first_upper(value: &str) -> String {
    let mut chars = value.chars();
    let first = match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>(),
        None => String::new(),
    };
    let rest: Vec<char> = chars.collect();
    let mut out = String::new();
    let mut i = 0;
    while i < rest.len() {
        if rest[i] == '_' && i + 1 < rest.len() && rest[i + 1].is_ascii_lowercase() {
            out.extend(rest[i + 1].to_uppercase());
            i += 2;
        } else {
            out.push(rest[i]);
            i += 1;
        }
    }
    format!("{}{}", first, out)
}

fn generate_permission_enum(enum_values: &[J]) -> String {
    let mut entries: Vec<String> = Vec::new();
    for value in enum_values {
        if let J::Str(s) = value {
            let key = convert_permission_to_pascal_case(s);
            entries.push(format!("  {} = {}", key, ts_str_lit(s)));
        }
    }
    format!("export enum Permission {{\n{}\n}}", entries.join(",\n"))
}

fn convert_permission_to_pascal_case(permission: &str) -> String {
    if permission == "*" {
        return "All".to_string();
    }
    permission
        .split("::")
        .map(|part| {
            part.split('-')
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join("")
        })
        .collect::<Vec<_>>()
        .join("")
}

fn generate_all_schemas(schemas: &IndexMap<String, J>) -> String {
    let mut interfaces: Vec<String> = Vec::new();
    let mut sorted_names: Vec<&String> = schemas.keys().collect();
    sorted_names.sort();

    for schema_name in sorted_names {
        if schema_name == "AnyType" || schema_name == "BlobType" {
            continue;
        }
        let schema = &schemas[schema_name];
        let def = generate_schema_interface(schema_name, schema);
        if !def.trim().is_empty() {
            interfaces.push(def);
        }
    }

    interfaces.join("\n\n")
}

fn generate_permissions_enum(permissions: &[PermissionInfo]) -> String {
    if permissions.is_empty() {
        return "export enum Permissions {}".to_string();
    }
    let entries: Vec<String> = permissions
        .iter()
        .map(|p| format!("  {} = {}", p.name, ts_str_lit(&p.value)))
        .collect();
    format!("export enum Permissions {{\n{}\n}}", entries.join(",\n"))
}

fn generate_permission_descriptions(permissions: &[PermissionInfo]) -> String {
    if permissions.is_empty() {
        return "export const PermissionDescriptions: Record<string, string> = {}".to_string();
    }
    let entries: Vec<String> = permissions
        .iter()
        .map(|p| format!("  {}: {}", p.name, ts_str_lit(&p.description)))
        .collect();
    format!(
        "export const PermissionDescriptions: Record<string, string> = {{\n{}\n}}",
        entries.join(",\n")
    )
}

const HEADER: &str = r#"/**
 * Generated API endpoint definitions
 * Auto-generated from OpenAPI specification
 * 
 * ⚠️  DO NOT EDIT THIS FILE MANUALLY ⚠️
 * This file is automatically generated from the OpenAPI specification generated from the server code.
 */

// =============================================================================
// TYPE DEFINITIONS
// =============================================================================

"#;

const HELPERS_SECTION: &str = r#"// Type helpers
// `ApiEndpoint`, `ApiEndpointUrl`, `getEndpointKey` moved to `./apiEndpoints`
// (imported above) so the `ApiEndpoints` value stays out of this eager module.

// Get parameter type for endpoint
export type GetParameterType<K extends ApiEndpoint> = ApiEndpointParameters[K]

// Get response type for endpoint  
export type GetResponseType<K extends ApiEndpoint> = ApiEndpointResponses[K]

// Create reverse mapping from URL to endpoint key
export type UrlToEndpoint<U extends ApiEndpointUrl> = {
  [K in keyof typeof ApiEndpoints]: (typeof ApiEndpoints)[K] extends U
    ? K
    : never
}[keyof typeof ApiEndpoints]

// Helper types to get parameter and response types by URL
export type ParameterByUrl<U extends ApiEndpointUrl> =
  ApiEndpointParameters[UrlToEndpoint<U>]
export type ResponseByUrl<U extends ApiEndpointUrl> =
  ApiEndpointResponses[UrlToEndpoint<U>]

// Type-safe validation - this will cause a TypeScript error if any endpoint is missing
type ValidateParametersComplete = {
  [K in keyof typeof ApiEndpoints]: K extends keyof ApiEndpointParameters
    ? true
    : false
}

type ValidateResponsesComplete = {
  [K in keyof typeof ApiEndpoints]: K extends keyof ApiEndpointResponses
    ? true
    : false
}

// Type-safe validation - these will cause a TypeScript error if any endpoint is missing
// from Parameters or Responses. They are used for compile-time validation only.
export type { ValidateParametersComplete, ValidateResponsesComplete }
"#;

fn generate_typescript_content(
    endpoints: &IndexMap<String, String>,
    parameters: &IndexMap<String, String>,
    responses: &IndexMap<String, String>,
    schemas: &IndexMap<String, J>,
    permissions: &[PermissionInfo],
) -> String {
    let mut sorted_endpoints: Vec<&String> = endpoints.keys().collect();
    sorted_endpoints.sort();

    let schema_definitions = format!("{}\n\n", generate_all_schemas(schemas));

    // NOTE: the `Permissions` enum and `PermissionDescriptions` map are emitted
    // as SEPARATE modules (`permissions.ts` / `permissionDescriptions.ts`) — see
    // `generate_permissions_ts` / `generate_permission_descriptions_ts`. Keeping
    // them out of this eagerly-imported `types.ts` lets their value uses inline
    // (permissions) and the objects land in lazy chunks, so the entry chunk stays
    // flat as the permission set grows. `permissions` param unused here now.
    let _ = permissions;
    let permissions_section = String::new();

    // `ApiEndpoints` (the value) + `ApiEndpoint`/`ApiEndpointUrl` + `getEndpointKey`
    // live in the split-out `apiEndpoints.ts`. This file keeps the endpoint-keyed
    // helper TYPES below, which reference `typeof ApiEndpoints` / `ApiEndpoint` —
    // imported here type-only (erased at build, no runtime tether).
    let endpoints_section = "import { ApiEndpoints } from './apiEndpoints'\nimport type { ApiEndpoint, ApiEndpointUrl } from './apiEndpoints'\n\n".to_string();

    let parameters_section = format!(
        "// API endpoint parameters\nexport type ApiEndpointParameters = {{\n{}\n}}\n\n",
        sorted_endpoints
            .iter()
            .map(|k| format!("  '{}': {}", k, parameters[*k]))
            .collect::<Vec<_>>()
            .join("\n")
    );

    let responses_section = format!(
        "// API endpoint responses\nexport type ApiEndpointResponses = {{\n{}\n}}\n\n",
        sorted_endpoints
            .iter()
            .map(|k| format!("  '{}': {}", k, responses[*k]))
            .collect::<Vec<_>>()
            .join("\n")
    );

    format!(
        "{}{}{}{}{}{}{}",
        HEADER,
        endpoints_section, // the `./apiEndpoints` import — must precede the type helpers
        schema_definitions,
        permissions_section,
        parameters_section,
        responses_section,
        HELPERS_SECTION
    )
}

/// Generate the full `types.ts` content from a parsed OpenAPI spec.
pub fn generate_types_ts(spec: &J) -> String {
    let mut endpoints: IndexMap<String, String> = IndexMap::new();
    let mut parameters: IndexMap<String, String> = IndexMap::new();
    let mut responses: IndexMap<String, String> = IndexMap::new();

    if let Some(paths) = spec.get("paths").and_then(|p| p.as_object()) {
        for (path, methods) in paths {
            if let Some(methods) = methods.as_object() {
                for (method, operation) in methods {
                    let operation_id = match operation.get("operationId").and_then(|x| x.as_str()) {
                        Some(id) => id.to_string(),
                        None => continue,
                    };
                    let http_method = method.to_uppercase();
                    // The TS does path.replace(/{([^}]+)}/g, '{$1}') — a no-op.
                    let api_path = path.clone();
                    endpoints.insert(operation_id.clone(), format!("{} {}", http_method, api_path));
                    parameters
                        .insert(operation_id.clone(), generate_parameter_type(operation, path));
                    responses.insert(
                        operation_id.clone(),
                        generate_response_type(operation, Some(&http_method)),
                    );
                }
            }
        }
    }

    let permissions = extract_permissions_from_spec(spec);

    let empty = IndexMap::new();
    let schemas = spec
        .get("components")
        .and_then(|c| c.get("schemas"))
        .and_then(|s| s.as_object())
        .unwrap_or(&empty);

    generate_typescript_content(&endpoints, &parameters, &responses, schemas, &permissions)
}

/// Parse an `openapi.json` string and emit `types.ts` content.
pub fn generate_types_ts_from_json(
    openapi_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let spec: J = serde_json::from_str(openapi_json)?;
    Ok(generate_types_ts(&spec))
}

/// Generate the standalone `permissionDescriptions.ts` module content. Split out
/// of `types.ts` so this human-readable label map (consumed only by the lazy
/// permission-picker UI) rides that lazy chunk instead of the eager entry chunk.
pub fn generate_permission_descriptions_ts(spec: &J) -> String {
    let permissions = extract_permissions_from_spec(spec);
    format!(
        "// AUTO-GENERATED — do not edit. Regenerate with `just openapi-regen`.\n// Human-readable permission labels, split out of `types.ts` so they load only\n// where used (the permission picker) instead of on every page.\n\n{}\n",
        generate_permission_descriptions(&permissions)
    )
}

/// Parse an `openapi.json` string and emit `permissionDescriptions.ts` content.
pub fn generate_permission_descriptions_ts_from_json(
    openapi_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let spec: J = serde_json::from_str(openapi_json)?;
    Ok(generate_permission_descriptions_ts(&spec))
}

/// Generate the standalone `permissions.ts` module (the `Permissions` enum),
/// split out of `types.ts`. Value uses (`Permissions.X`) are inlined to string
/// literals by the app's build transform, so in a fully-inlined build this
/// module is pulled only by the (lazy) permission picker that enumerates it —
/// keeping the enum object out of the eager entry chunk. Scales flat as the
/// permission set grows.
pub fn generate_permissions_ts(spec: &J) -> String {
    let permissions = extract_permissions_from_spec(spec);
    format!(
        "// AUTO-GENERATED — do not edit. Regenerate with `just openapi-regen`.\n// The `Permissions` enum, split out of `types.ts` so it isn't pinned to the\n// eager entry chunk (value uses are inlined at build; only the lazy picker\n// enumerates the object).\n\n{}\n",
        generate_permissions_enum(&permissions)
    )
}

/// Parse an `openapi.json` string and emit `permissions.ts` content.
pub fn generate_permissions_ts_from_json(
    openapi_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let spec: J = serde_json::from_str(openapi_json)?;
    Ok(generate_permissions_ts(&spec))
}

/// Generate the standalone `apiEndpoints.ts` module: the `ApiEndpoints` map (a
/// full enumeration of the API surface) + `ApiEndpoint`/`ApiEndpointUrl` types +
/// `getEndpointKey`. Split out of `types.ts` so the app's build transform can
/// inline every `ApiClient.NS.method()` call to `callAsync("METHOD /url", …)` and
/// the map itself tree-shakes away — no single API inventory ships, and the entry
/// chunk stays flat as endpoints grow. `types.ts` keeps the endpoint-keyed helper
/// TYPES (they reference `typeof ApiEndpoints`, an erased import).
pub fn generate_api_endpoints_ts(spec: &J) -> String {
    let mut endpoints: IndexMap<String, String> = IndexMap::new();
    if let Some(paths) = spec.get("paths").and_then(|p| p.as_object()) {
        for (path, methods) in paths {
            if let Some(methods) = methods.as_object() {
                for (method, operation) in methods {
                    let operation_id = match operation.get("operationId").and_then(|x| x.as_str())
                    {
                        Some(id) => id.to_string(),
                        None => continue,
                    };
                    endpoints
                        .insert(operation_id, format!("{} {}", method.to_uppercase(), path));
                }
            }
        }
    }
    let mut sorted: Vec<&String> = endpoints.keys().collect();
    sorted.sort();
    let body = sorted
        .iter()
        .map(|k| format!("  '{}': '{}'", k, endpoints[*k]))
        .collect::<Vec<_>>()
        .join(",\n");
    format!(
        "// AUTO-GENERATED — do not edit. Regenerate with `just openapi-regen`.\n// The API endpoint map, split out of `types.ts`. `ApiClient.NS.method()` calls\n// are inlined to `callAsync(\"METHOD /url\", …)` at build, so this map ships only\n// where still enumerated (e.g. `getEndpointKey`), not in the entry chunk.\n\nexport const ApiEndpoints = {{\n{}\n}} as const\n\nexport type ApiEndpoint = keyof typeof ApiEndpoints\nexport type ApiEndpointUrl = (typeof ApiEndpoints)[ApiEndpoint]\n\n// Extract endpoint key from URL pattern\nexport function getEndpointKey(url: string): ApiEndpoint | undefined {{\n  const entries = Object.entries(ApiEndpoints) as [ApiEndpoint, string][]\n  const found = entries.find(([_key, value]) => value === url)\n  return found ? found[0] : undefined\n}}\n",
        body
    )
}

/// Parse an `openapi.json` string and emit `apiEndpoints.ts` content.
pub fn generate_api_endpoints_ts_from_json(
    openapi_json: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let spec: J = serde_json::from_str(openapi_json)?;
    Ok(generate_api_endpoints_ts(&spec))
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_string_literals() {
        assert_eq!(ts_str_lit("plain"), "'plain'");
        assert_eq!(ts_str_lit("it's"), "'it\\'s'");
        assert_eq!(ts_str_lit("a\\b"), "'a\\\\b'");
        // backslash escaped before quote so it isn't double-processed
        assert_eq!(ts_str_lit("a\\'b"), "'a\\\\\\'b'");
    }

    #[test]
    fn quotes_non_identifier_member_keys() {
        assert_eq!(ts_prop_key("snake_case"), "snake_case");
        assert_eq!(ts_prop_key("$ok"), "$ok");
        assert_eq!(ts_prop_key("default"), "default"); // reserved word ok as member
        assert_eq!(ts_prop_key("foo-bar"), "'foo-bar'");
        assert_eq!(ts_prop_key("2fa"), "'2fa'");
        assert_eq!(ts_prop_key("with space"), "'with space'");
    }

    /// GENERATOR-CORRECTNESS golden gate (moved from ziee's per-app parity test
    /// in Chunk B6). A small committed fixture `openapi.json` must produce the
    /// committed expected `types.ts` **byte-for-byte**. This proves the relocated
    /// generator is a faithful pure function of its input, independent of any
    /// app. The per-app *regen-drift* guard (does ziee's real `types.ts` match
    /// its `openapi.json`?) stays app-side in ziee's `openapi` module.
    #[test]
    fn generator_golden_fixture() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/openapi/fixtures");
        let openapi_path = format!("{}/openapi.json", dir);
        let golden_path = format!("{}/types.ts", dir);

        let openapi = std::fs::read_to_string(&openapi_path)
            .unwrap_or_else(|e| panic!("read {}: {}", openapi_path, e));
        let golden = std::fs::read_to_string(&golden_path)
            .unwrap_or_else(|e| panic!("read {}: {}", golden_path, e));

        let generated = generate_types_ts_from_json(&openapi).expect("generate");

        if generated != golden {
            let g: Vec<&str> = generated.lines().collect();
            let e: Vec<&str> = golden.lines().collect();
            let mut first = None;
            for i in 0..g.len().max(e.len()) {
                if g.get(i) != e.get(i) {
                    first = Some(i);
                    break;
                }
            }
            if let Some(i) = first {
                let lo = i.saturating_sub(2);
                let mut msg = format!(
                    "generator golden fixture mismatch at line {} (generated {} lines, golden {} lines)\n",
                    i + 1,
                    g.len(),
                    e.len()
                );
                for j in lo..=(i + 2) {
                    msg.push_str(&format!(
                        "  {}: GEN |{}|\n      GOLD|{}|\n",
                        j + 1,
                        g.get(j).unwrap_or(&"<EOF>"),
                        e.get(j).unwrap_or(&"<EOF>")
                    ));
                }
                panic!("{}", msg);
            }
            panic!("generator golden fixture mismatch (trailing-content difference)");
        }
    }
}
