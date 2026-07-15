use schemars::JsonSchema;
use serde::Serialize;

/// Cached server update-availability status (admin endpoint).
#[derive(Clone, Debug, Serialize, JsonSchema, Default)]
pub struct UpdateStatusResponse {
    /// The running server version (`CARGO_PKG_VERSION`).
    pub current_version: String,
    /// Latest version seen on GitHub, or null if not yet checked / disabled.
    pub latest_version: Option<String>,
    /// True when `latest_version` is newer than `current_version`.
    pub update_available: bool,
    /// GitHub release page for the latest version.
    pub release_url: Option<String>,
    /// Release notes (markdown) for the latest version.
    pub notes: Option<String>,
    /// RFC3339 timestamp of the last successful check, or null if never.
    pub checked_at: Option<String>,
    /// Whether update checks are enabled in config (false → air-gapped or the
    /// embedded desktop server).
    pub enabled: bool,
}

#[cfg(test)]
mod tests {
    use super::UpdateStatusResponse;

    /// `Default` yields the "never checked / disabled" resting state the admin
    /// endpoint returns before the first poll: no latest version, not available,
    /// not checked, disabled.
    #[test]
    fn default_is_the_never_checked_state() {
        let d = UpdateStatusResponse::default();
        assert_eq!(d.current_version, "");
        assert!(d.latest_version.is_none());
        assert!(!d.update_available);
        assert!(d.release_url.is_none());
        assert!(d.notes.is_none());
        assert!(d.checked_at.is_none());
        assert!(!d.enabled);
    }

    /// The serialized field names are the wire contract consumed by the admin UI
    /// (and pinned by the schemars key). A rename here silently breaks the client
    /// — pin the exact snake_case field set + values.
    #[test]
    fn serializes_with_stable_snake_case_field_names() {
        let r = UpdateStatusResponse {
            current_version: "1.2.3".to_string(),
            latest_version: Some("1.3.0".to_string()),
            update_available: true,
            release_url: Some("https://example/releases/1.3.0".to_string()),
            notes: Some("bugfixes".to_string()),
            checked_at: Some("2026-07-14T00:00:00Z".to_string()),
            enabled: true,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["current_version"], "1.2.3");
        assert_eq!(v["latest_version"], "1.3.0");
        assert_eq!(v["update_available"], true);
        assert_eq!(v["release_url"], "https://example/releases/1.3.0");
        assert_eq!(v["notes"], "bugfixes");
        assert_eq!(v["checked_at"], "2026-07-14T00:00:00Z");
        assert_eq!(v["enabled"], true);
        // Exactly these seven keys, no extras/renames.
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 7, "unexpected field set: {:?}", obj.keys().collect::<Vec<_>>());
    }

    /// Option fields serialize as JSON null (not omitted) when None — the UI
    /// reads them by presence, so the shape must stay stable.
    #[test]
    fn none_options_serialize_as_null() {
        let v = serde_json::to_value(UpdateStatusResponse::default()).unwrap();
        assert!(v["latest_version"].is_null());
        assert!(v["release_url"].is_null());
        assert!(v["checked_at"].is_null());
    }
}
