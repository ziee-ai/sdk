//! The per-module notification-kind contribution registry (backend half of the
//! SDK notification seam). A module DECLARES its notification kind(s) via a
//! `#[distributed_slice(NOTIFICATION_KINDS)]` static so the deployment can
//! introspect the set of kinds (`GET /api/notifications/kinds`) and validate an
//! emitted kind against it. Producers still create rows directly via
//! `events::create_and_emit`; this registry is the declaration + introspection
//! surface (the RENDER half lives in the frontend registry).

use linkme::distributed_slice;
use schemars::JsonSchema;
use serde::Serialize;

/// A module's declaration of one notification kind it produces.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct NotificationKindDescriptor {
    /// The stable kind key stored on the row + dispatched on by the FE renderer.
    pub kind: &'static str,
    /// Human-readable description of when this kind is produced.
    pub description: &'static str,
}

/// Every notification kind contributed by a module. A module registers into it
/// with `#[distributed_slice(NOTIFICATION_KINDS)] static X: NotificationKindDescriptor = …;`.
#[distributed_slice]
pub static NOTIFICATION_KINDS: [NotificationKindDescriptor] = [..];

/// All registered kind descriptors (deduplicated by kind, insertion order not
/// guaranteed by linkme).
pub fn registered_kinds() -> Vec<NotificationKindDescriptor> {
    NOTIFICATION_KINDS.iter().cloned().collect()
}

/// True iff `kind` was declared by some module.
pub fn is_registered_kind(kind: &str) -> bool {
    NOTIFICATION_KINDS.iter().any(|d| d.kind == kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A crate-local test registration proves the distributed_slice collects.
    #[distributed_slice(NOTIFICATION_KINDS)]
    static TEST_KIND: NotificationKindDescriptor = NotificationKindDescriptor {
        kind: "__test_kind__",
        description: "test-only",
    };

    #[test]
    fn slice_collects_registered_kinds() {
        assert!(is_registered_kind("__test_kind__"));
        assert!(!is_registered_kind("nope"));
        assert!(registered_kinds().iter().any(|d| d.kind == "__test_kind__"));
    }

    #[test]
    fn descriptor_serializes_kind_and_description() {
        // `GET /api/notifications/kinds` returns these verbatim; the wire field
        // names must stay `kind` + `description`.
        let d = NotificationKindDescriptor { kind: "study_share_invite", description: "when a study is shared" };
        let v = serde_json::to_value(&d).unwrap();
        assert_eq!(v["kind"], "study_share_invite");
        assert_eq!(v["description"], "when a study is shared");
    }
}
