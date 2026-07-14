// @ziee/framework/permissions — framework-level permission-gating primitives.
//
// The four gating layers an app composes on top of these: slot `permission`
// fields → route `permission` (via `@ziee/framework/router`'s RoutePermissionGate)
// → `<Can>` → `usePermission`. See ziee's `.claude/PERMISSION_GATING.md`.
//
// SEAM: the reactive/snapshot hooks read the app-registered `Auth` store (a
// store named `Auth` exposing `{ user, permissions }`) via `authStoreProxy`.

export { Can } from './Can'
export { evaluatePermission } from './evaluatePermission'
export { hasPermission } from './hasPermission'
export { hasPermissionNow } from './hasPermissionNow'
export { usePermission } from './usePermission'
export { authStoreProxy } from './authView'
export type { PermissionAuthView } from './authView'
export type { Permission, PermissionExpr, PermissionUser } from './types'
