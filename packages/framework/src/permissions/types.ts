// Framework-level permission primitives. Used by every gating surface (slot
// fields, <Can>, usePermission, the evaluator). App-agnostic: the leaf type is
// a bare `string`, and an app narrows it to its own generated `Permissions`
// enum at its own re-export boundary if it wants enum-level type-safety.
//
// See ziee's `.claude/PERMISSION_GATING.md` for the four-layer gating pattern.

/**
 * A single permission leaf. Framework-level, so this is a bare `string` —
 * matched with the same wildcard rules as the backend (`*`,
 * `module::resource::*`, etc.). An app may re-export a narrowed alias
 * (`Permission = MyPermissionsEnum`); enum members are assignable to `string`
 * so the primitives accept both.
 */
export type Permission = string

/**
 * Composable permission expression.
 *
 * - bare leaf: an exact permission, wildcard-matched (`*`, `module::resource::*`).
 * - `allOf`: every child expression must pass (AND). Empty is vacuously true.
 * - `anyOf`: at least one child expression must pass (OR). Empty is false.
 *
 * The shape is intentionally serializable (no functions) so it can flow through
 * slot registrations and be inspected by tooling.
 */
export type PermissionExpr =
  | Permission
  | { allOf: PermissionExpr[] }
  | { anyOf: PermissionExpr[] }

/**
 * The minimal shape the permission primitives read off the current user: the
 * root-admin bypass flag. An app's full `User` type is structurally assignable
 * to this (it carries `is_admin`), so callers pass their `User` directly.
 */
export interface PermissionUser {
  is_admin?: boolean | null
}
