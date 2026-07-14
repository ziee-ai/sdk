import { hasPermission } from './hasPermission'
import type { PermissionExpr, PermissionUser } from './types'

/**
 * Walk a `PermissionExpr` tree and evaluate it against the user's permissions.
 * Bare strings delegate to `hasPermission` (the leaf check). `allOf` is AND,
 * `anyOf` is OR. Empty `allOf` is vacuously true; empty `anyOf` is false.
 */
export function evaluatePermission(
  user: PermissionUser | null | undefined,
  permissions: string[] | null | undefined,
  expr: PermissionExpr,
): boolean {
  // Defensive: an undefined / null expression means "fail closed" (no grant).
  // Happens when an enum lookup resolves to undefined (e.g. a consumer's
  // api-client types are stale and missing the constant). Without this guard,
  // the next `'allOf' in expr` throws "Cannot use 'in' operator … in undefined"
  // and crashes the whole router.
  if (expr == null) {
    return false
  }
  if (typeof expr === 'string') {
    return hasPermission(user, permissions, expr)
  }
  if ('allOf' in expr) {
    return expr.allOf.every(child =>
      evaluatePermission(user, permissions, child),
    )
  }
  if ('anyOf' in expr) {
    return expr.anyOf.some(child => evaluatePermission(user, permissions, child))
  }
  return false
}
