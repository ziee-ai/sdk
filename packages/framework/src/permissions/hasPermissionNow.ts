import { authStoreProxy } from './authView'
import { evaluatePermission } from './evaluatePermission'
import type { PermissionExpr } from './types'

/**
 * Non-reactive permission check, safe to call from store `init` hooks, actions,
 * event handlers, or any code path outside a React component body.
 *
 * Reads `user` + `permissions` from the app-registered `Auth` store's `.$`
 * snapshot, so it does NOT subscribe to changes. If a component needs to
 * re-render when permissions change, use `usePermission()` / `<Can>` instead.
 *
 * Primary use case: gating shell-eager-load fetches — modules whose `init`
 * calls `/api/...` for resources the user may not have access to. Without the
 * gate, the shell 403s on every render for permission-restricted users.
 */
export function hasPermissionNow(expr: PermissionExpr): boolean {
  const { user, permissions } = authStoreProxy().$
  return evaluatePermission(user, permissions, expr)
}
