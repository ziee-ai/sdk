import { authStoreProxy } from './authView'
import { evaluatePermission } from './evaluatePermission'
import type { PermissionExpr } from './types'

/**
 * Returns whether the current authenticated user satisfies the given permission
 * expression. Reads `user` + `permissions` reactively from the app-registered
 * `Auth` store (see {@link authStoreProxy}).
 *
 * Composition lives in the expression, not the hook name — pass
 * `{ allOf: [...] }` for AND or `{ anyOf: [...] }` for OR rather than reaching
 * for separate `useAllPermissions` / `useAnyPermission` hooks.
 */
export function usePermission(expr: PermissionExpr): boolean {
  const { user, permissions } = authStoreProxy()
  return evaluatePermission(user, permissions, expr)
}
