import type { StoreProxy } from '../stores'
import type { PermissionUser } from './types'

/**
 * The identity slice the permission hooks read: the current user (for the
 * `is_admin` bypass) + their flattened active-group permission strings.
 *
 * SEAM (injection): a consuming app calls `setAuthView(Auth)` once with its
 * `Auth` store proxy (exposing at least `{ user, permissions }`). The permission
 * primitives read it through this typed view rather than importing the app's
 * concrete Auth store OR going through a global `Stores.Auth` — so the framework
 * stays app-agnostic with zero global-registry dependency.
 */
export interface PermissionAuthView {
  user: PermissionUser | null | undefined
  permissions: string[] | null | undefined
}

let injectedAuthView: StoreProxy<PermissionAuthView> | null = null

/** App entry point: register the `Auth` store proxy for the permission system. */
export function setAuthView(view: StoreProxy<PermissionAuthView>): void {
  injectedAuthView = view
}

/**
 * The app-registered `Auth` store proxy, viewed as {@link PermissionAuthView}.
 * Reading a field off the returned proxy inside a React render subscribes to it
 * (reactive); read `.$` for a non-reactive snapshot.
 */
export function authStoreProxy(): StoreProxy<PermissionAuthView> {
  if (!injectedAuthView) {
    throw new Error(
      '[permissions] Auth view not registered — the app must call setAuthView(Auth) at startup',
    )
  }
  return injectedAuthView
}
