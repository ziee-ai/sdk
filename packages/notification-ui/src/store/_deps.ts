import type { NotificationsStoreDeps } from '../types'

/**
 * The app-injected dependencies (REST surface + read permission), held at module
 * scope so the globbed `actions/*.ts` files — which are their own modules and
 * only receive `set`/`get` — can reach them. `createNotificationsStore(deps)`
 * populates this once per process (there is exactly ONE Notifications store per
 * app). The passthrough VALUES (`onNavigate`/`inboxPath`) live in state instead,
 * since the bell/inbox read them off the store view.
 */
let deps: NotificationsStoreDeps | null = null

export function setNotificationDeps(d: NotificationsStoreDeps): void {
  deps = d
}

export function notificationDeps(): NotificationsStoreDeps {
  if (!deps) {
    throw new Error(
      '[notifications] store deps not initialized — createNotificationsStore must run first',
    )
  }
  return deps
}
