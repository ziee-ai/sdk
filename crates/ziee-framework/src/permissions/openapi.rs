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
pub fn with_permission<Perms: PermissionList>(op: TransformOperation) -> TransformOperation {
    // Add description with permission info
    let permission_desc = Perms::format_description();

    let op = op.description(&permission_desc);

    // Add standard 403 Forbidden response with JSON body
    let names = Perms::names();
    let permissions = Perms::permissions();
    let descriptions = Perms::descriptions();

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
    }
}
