import type { Permission, PermissionUser } from './types'

/**
 * Leaf permission check — mirrors the backend's
 * `permissions/checker.rs::check_permissions_array` exactly, plus the
 * `is_admin` short-circuit that the backend applies one level up at
 * `permissions/extractors.rs`.
 *
 * Order matters:
 * 1. `user.is_admin === true` → granted (root admin bypass — the /api/auth/me
 *    payload does NOT rewrite permissions[] to ["*"] for root admins, so we
 *    must short-circuit here).
 * 2. exact match in permissions[].
 * 3. `*` global wildcard in permissions[].
 * 4. hierarchical `::` wildcard: for `a::b::c`, check `a::*` and `a::b::*`.
 *    Separator is double-colon, matching the backend.
 */
export function hasPermission(
  user: PermissionUser | null | undefined,
  permissions: string[] | null | undefined,
  required: Permission,
): boolean {
  if (user?.is_admin) return true

  if (!permissions || permissions.length === 0) return false

  if (permissions.includes(required)) return true
  if (permissions.includes('*')) return true

  const parts = required.split('::')
  for (let i = 1; i < parts.length; i++) {
    const wildcard = parts.slice(0, i).join('::') + '::*'
    if (permissions.includes(wildcard)) return true
  }

  return false
}
