//! OpenAPI decoration for permission-gated operations — moved from ziee's
//! `modules/permissions/openapi.rs` in chunk B3. Generic over
//! [`ziee_identity::PermissionList`], it never names a concrete permission.
//! The `PermissionError` schema types keep their names (schemars uses the type's
//! short name, not its crate path), so the emitted OpenAPI 403 schema is
//! byte-identical after the move (E8-neutral).

use aide::transform::TransformOperation;
use axum::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use ziee_identity::PermissionList;

/// 403 Forbidden response for missing permissions
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PermissionError {
    pub error: String,
    pub error_code: String,
    pub details: PermissionErrorDetails,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionErrorDetails {
    pub required_permissions: Vec<PermissionDetail>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PermissionDetail {
    pub name: String,
    pub value: String,
    pub description: String,
}

/// Helper function to add permission info to OpenAPI operations
///
/// This enhances the OpenAPI spec with:
/// - Enhanced descriptions mentioning the required permission
/// - Proper 403 Forbidden response documentation
/// - Security requirement for Bearer token
///
/// # Example
/// ```rust,ignore
/// use ziee_framework::permissions::with_permission;
///
/// fn list_users_docs(op: TransformOperation) -> TransformOperation {
///     with_permission::<(UsersRead,)>(op)
///         .tag("Admin - Users")
///         .summary("List all users with pagination")
/// }
/// ```
/// OpenAPI extension key carrying the operation's required permissions.
///
/// This is the ONLY clobber-proof record of them. The human-readable copy lives
/// in the operation's `description`, which any later `.description("…")` in the
/// same `_docs` builder REPLACES; the machine-readable copy in the 403 example
/// is likewise lost when a builder attaches its own `.response_with::<403, …>`.
/// Measured on the shipped spec, those two losses left 201 and then 18
/// permission-gated operations with no recoverable permission — which the
/// control MCP catalog reads as "no permission declared → anyone may run it".
/// An `x-` extension is written by nobody else and survives both.
pub const X_REQUIRED_PERMISSIONS: &str = "x-required-permissions";

pub fn with_permission<Perms: PermissionList>(op: TransformOperation) -> TransformOperation {
    // Add description with permission info
    let permission_desc = Perms::format_description();

    let mut op = op.description(&permission_desc);

    // Add standard 403 Forbidden response with JSON body
    let names = Perms::names();
    let permissions = Perms::permissions();
    let descriptions = Perms::descriptions();

    // Stamp the machine-readable, unclobberable record. ALL permissions, in
    // declaration order — the extractor requires every one of them.
    op.inner_mut().extensions.insert(
        X_REQUIRED_PERMISSIONS.to_string(),
        serde_json::Value::Array(
            permissions
                .iter()
                .map(|p| serde_json::Value::String((*p).to_string()))
                .collect(),
        ),
    );
    let op = op;

    // Create example with permissions
    let error_msg = if permissions.len() == 1 {
        format!("Missing required permission: {}", permissions[0])
    } else {
        format!("Missing required permissions: {}", permissions.join(", "))
    };

    let permission_details: Vec<PermissionDetail> = names
        .iter()
        .zip(permissions.iter())
        .zip(descriptions.iter())
        .map(|((name, perm), desc)| PermissionDetail {
            name: name.to_string(),
            value: perm.to_string(),
            description: desc.to_string(),
        })
        .collect();

    let example_details = PermissionErrorDetails {
        required_permissions: permission_details,
    };

    let op = op.response_with::<403, Json<PermissionError>, _>(move |res| {
        res.description("Forbidden - Missing required permission")
            .example(PermissionError {
                error: error_msg.clone(),
                error_code: "INSUFFICIENT_PERMISSIONS".to_string(),
                details: example_details.clone(),
            })
    });

    // Add security requirement for Bearer token
    op.security_requirement("bearerAuth")
}

#[cfg(test)]
mod with_permission_tests {
    use super::*;
    use ziee_identity::PermissionCheck;

    struct TestPerm;
    impl PermissionCheck for TestPerm {
        const NAME: &'static str = "UsersRead";
        const PERMISSION: &'static str = "users::read";
        const DESCRIPTION: &'static str = "Read users";
        const MODULE: &'static str = "users";
    }

    /// `with_permission` decorates the OpenAPI operation with the bearer-auth
    /// security requirement, a documented 403 (INSUFFICIENT_PERMISSIONS), and a
    /// description naming the required permission. Asserted via the serialized
    /// operation to avoid coupling to aide's internal types.
    #[test]
    fn with_permission_documents_403_bearer_and_permission() {
        let mut op = aide::openapi::Operation::default();
        {
            let t = TransformOperation::new(&mut op);
            let _ = with_permission::<(TestPerm,)>(t);
        }
        let json = serde_json::to_value(&op).expect("serialize operation");

        // A 403 response is documented.
        assert!(
            json["responses"]["403"].is_object(),
            "with_permission must document a 403 response: {json}"
        );
        // The bearer-auth security requirement was added.
        assert!(
            serde_json::to_string(&json["security"])
                .unwrap()
                .contains("bearerAuth"),
            "with_permission must add the bearerAuth security requirement: {}",
            json["security"]
        );
        // The description names the required permission.
        assert!(
            json["description"].as_str().unwrap_or("").contains("users::read"),
            "with_permission must name the permission in the description: {}",
            json["description"]
        );
        // …and the unclobberable extension carries it too.
        assert_eq!(
            json[X_REQUIRED_PERMISSIONS],
            serde_json::json!(["users::read"]),
            "with_permission must stamp {X_REQUIRED_PERMISSIONS}: {json}"
        );
    }

    struct TestPerm2;
    impl PermissionCheck for TestPerm2 {
        const NAME: &'static str = "UsersEdit";
        const PERMISSION: &'static str = "users::edit";
        const DESCRIPTION: &'static str = "Edit users";
        const MODULE: &'static str = "users";
    }

    /// The extension carries EVERY permission of an ALL-of operation, in
    /// declaration order — the description's multi-permission heading is not
    /// even parseable by the catalog, and taking only the first would under-gate.
    #[test]
    fn extension_carries_all_permissions_of_an_all_of_operation() {
        let mut op = aide::openapi::Operation::default();
        {
            let t = TransformOperation::new(&mut op);
            let _ = with_permission::<(TestPerm, TestPerm2)>(t);
        }
        let json = serde_json::to_value(&op).expect("serialize operation");
        assert_eq!(
            json[X_REQUIRED_PERMISSIONS],
            serde_json::json!(["users::read", "users::edit"]),
            "got {json}"
        );
    }

    /// The extension survives a handler `_docs` builder that REPLACES both the
    /// description and the 403 response — the exact shape that lost the
    /// permission on 201 + 18 operations.
    #[test]
    fn extension_survives_a_description_and_403_override() {
        let mut op = aide::openapi::Operation::default();
        {
            let t = TransformOperation::new(&mut op);
            let t = with_permission::<(TestPerm,)>(t);
            // What a real handler does afterwards:
            let t = t.description("Create a personal chat project.");
            let _ = t.response_with::<403, (), _>(|r| r.description("Not the owner"));
        }
        let json = serde_json::to_value(&op).expect("serialize operation");
        assert!(
            !json["description"].as_str().unwrap_or("").contains("users::read"),
            "precondition: the description must have been clobbered: {json}"
        );
        assert_eq!(
            json[X_REQUIRED_PERMISSIONS],
            serde_json::json!(["users::read"]),
            "the extension must survive: {json}"
        );
    }
}
