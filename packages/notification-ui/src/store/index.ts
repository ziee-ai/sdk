import { hasPermissionNow } from '@ziee/framework/permissions'
import { defineStore } from '@ziee/framework/store-kit'

import type { NotificationsStoreDeps } from '../types'
import { setNotificationDeps } from './_deps'
import { notificationsState, type NotificationsState } from './state'
import type { Actions } from './actions.gen'

/**
 * The notification inbox store — GENERIC and app-agnostic, folder-glob lazy-store
 * pattern (`state.ts` + `actions/*.ts` + this index). Loads the paged inbox +
 * unread count, and subscribes to `sync:notification` (+ `sync:reconnect`) to
 * refetch live. Owner-scoped on the server; every fetch self-gates on the
 * injected `readPermission` (the no-403 invariant — same perm the endpoint
 * enforces).
 *
 * The app binds its concrete REST surface + permission via `deps` — held in
 * `./_deps` so the globbed action modules can reach them (see `_deps.ts`). The
 * passthrough `onNavigate`/`inboxPath` are seeded into state (read off the store
 * view by the prop-less bell/inbox widgets). Returns the `defineStore` handle so
 * the app's thin consumer re-exports `{ Notifications, useNotificationsStore }`
 * and registers it under `Stores.Notifications`.
 */
export function createNotificationsStore(deps: NotificationsStoreDeps) {
  setNotificationDeps(deps)
  return defineStore<NotificationsState, Actions>('Notifications', {
    immer: true,
    state: {
      ...notificationsState,
      onNavigate: deps.onNavigate,
      inboxPath: deps.inboxPath,
    },
    actions: import.meta.glob('./actions/*.ts'),
    init: ({ on, actions }) => {
      const reload = () => {
        if (!hasPermissionNow(deps.readPermission)) return
        void actions.load()
      }
      on('sync:notification', reload)
      on('sync:reconnect', reload)
      reload()
    },
  })
}
