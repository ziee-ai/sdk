import { Stores } from '../stores'
import type { StoreProxy } from '../stores'
import type { PermissionUser } from './types'

/**
 * The identity slice the permission hooks read: the current user (for the
 * `is_admin` bypass) + their flattened active-group permission strings.
 *
 * SEAM: a consuming app registers a store named `Auth` (via the module system)
 * exposing at least `{ user, permissions }`. The permission primitives read it
 * through this typed view rather than importing the app's concrete Auth store —
 * so the framework stays app-agnostic while the runtime read is byte-identical
 * to reading `Stores.Auth` directly.
 */
export interface PermissionAuthView {
  user: PermissionUser | null | undefined
  permissions: string[] | null | undefined
}

/**
 * The app-registered `Auth` store proxy, viewed as {@link PermissionAuthView}.
 * Reading a field off the returned proxy inside a React render subscribes to it
 * (reactive); read `.$` for a non-reactive snapshot.
 */
export function authStoreProxy(): StoreProxy<PermissionAuthView> {
  return (Stores as unknown as { Auth: StoreProxy<PermissionAuthView> }).Auth
}
