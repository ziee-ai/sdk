//! `${VAR}` env templating for the declarative seed system.
//!
//! Secrets are NEVER inlined in a seed YAML: a secret field must be exactly one
//! `${VAR}` placeholder resolved from the process environment at apply time, and
//! resolved values are never logged (logs name the env VAR only). These are pure
//! functions with an injectable lookup so unit tests never mutate the process
//! environment (`std::env::set_var` is `unsafe` — it can realloc the shared
//! `environ` block under a concurrent `getenv` in another test thread).

#[derive(Debug, PartialEq, Eq)]
pub enum TemplateError {
    /// `${VAR}` had no value in the environment (or was empty).
    Unresolved(String),
    /// A secret field carried something other than a single `${VAR}`.
    InlineSecret,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::Unresolved(var) => write!(f, "env var ${{{var}}} is unset or empty"),
            TemplateError::InlineSecret => write!(
                f,
                "secret fields must be exactly one ${{ENV_VAR}} placeholder, never an inline value"
            ),
        }
    }
}

/// How a `${VAR}` is looked up. Production passes [`env_lookup`]; tests pass a map.
pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

/// The production lookup: process environment.
pub fn env_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Substitute every `${VAR}` in `raw` with its value from `lookup`.
///
/// A `$` not followed by `{` is left intact. An unset/EMPTY var is an error (the
/// caller skips that entry) rather than a silent empty string.
pub fn resolve_with(raw: &str, lookup: Lookup<'_>) -> Result<String, TemplateError> {
    let mut out = String::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            match raw[i + 2..].find('}') {
                Some(rel_end) => {
                    let name = &raw[i + 2..i + 2 + rel_end];
                    // `${}` (empty name) is not a placeholder — pass it through.
                    if name.is_empty() {
                        out.push_str("${}");
                    } else {
                        let value = lookup(name).unwrap_or_default();
                        if value.is_empty() {
                            return Err(TemplateError::Unresolved(name.to_string()));
                        }
                        out.push_str(&value);
                    }
                    i += 2 + rel_end + 1;
                }
                // Unterminated `${` — literal.
                None => {
                    out.push_str(&raw[i..]);
                    break;
                }
            }
        } else {
            let ch_len = raw[i..].chars().next().map(char::len_utf8).unwrap_or(1);
            out.push_str(&raw[i..i + ch_len]);
            i += ch_len;
        }
    }

    Ok(out)
}

/// Resolve a SECRET field. The value must be exactly one `${VAR}` placeholder;
/// anything else means a secret was committed inline, which we refuse.
pub fn resolve_secret_with(raw: &str, lookup: Lookup<'_>) -> Result<String, TemplateError> {
    let trimmed = raw.trim();
    let is_single_placeholder = trimmed.starts_with("${")
        && trimmed.ends_with('}')
        && trimmed.len() > 3
        && !trimmed[2..trimmed.len() - 1].contains('{')
        && !trimmed[2..trimmed.len() - 1].contains('}');

    if !is_single_placeholder {
        return Err(TemplateError::InlineSecret);
    }
    resolve_with(trimmed, lookup)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn map_lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| map.get(name).cloned()
    }

    #[test]
    fn resolve_substitutes_vars() {
        let lookup = map_lookup(&[("MCP_URL", "http://x:9000/mcp")]);
        assert_eq!(resolve_with("${MCP_URL}", &lookup).unwrap(), "http://x:9000/mcp");
        assert_eq!(
            resolve_with("a ${MCP_URL} b", &lookup).unwrap(),
            "a http://x:9000/mcp b"
        );
    }

    #[test]
    fn resolve_errors_on_unset_or_empty_var() {
        let lookup = map_lookup(&[("EMPTY", "")]);
        assert_eq!(
            resolve_with("${NOT_SET}", &lookup).unwrap_err(),
            TemplateError::Unresolved("NOT_SET".to_string())
        );
        assert!(matches!(
            resolve_with("${EMPTY}", &lookup).unwrap_err(),
            TemplateError::Unresolved(_)
        ));
    }

    #[test]
    fn resolve_leaves_non_placeholder_dollars_and_is_utf8_safe() {
        let lookup = map_lookup(&[("V", "x")]);
        assert_eq!(resolve_with("costs $5", &lookup).unwrap(), "costs $5");
        assert_eq!(resolve_with("${}", &lookup).unwrap(), "${}");
        assert_eq!(resolve_with("a ${OPEN", &lookup).unwrap(), "a ${OPEN");
        assert_eq!(resolve_with("héllo ${V} 日本", &lookup).unwrap(), "héllo x 日本");
    }

    #[test]
    fn resolve_secret_accepts_only_a_single_placeholder() {
        let lookup = map_lookup(&[("PW", "s3cret")]);
        assert_eq!(resolve_secret_with("${PW}", &lookup).unwrap(), "s3cret");
        assert_eq!(resolve_secret_with("  ${PW}  ", &lookup).unwrap(), "s3cret");
        for inline in ["hunter2", "prefix-${PW}", "${PW}${PW}", "${PW}x", ""] {
            assert_eq!(
                resolve_secret_with(inline, &lookup).unwrap_err(),
                TemplateError::InlineSecret,
                "{inline:?} must be rejected as an inline secret"
            );
        }
    }

    #[test]
    fn resolve_secret_propagates_an_unset_var() {
        let lookup = map_lookup(&[]);
        assert!(matches!(
            resolve_secret_with("${PW_UNSET}", &lookup).unwrap_err(),
            TemplateError::Unresolved(_)
        ));
    }
}
