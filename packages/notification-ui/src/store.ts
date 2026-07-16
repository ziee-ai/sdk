import { hasPermissionNow } from '@ziee/framework/permissions'
import { defineStore } from '@ziee/framework/store-kit'

import type { NotificationRow, NotificationsStoreDeps } from './types'

/**
 * The notification inbox store — GENERIC and app-agnostic. Loads the paged
 * inbox + unread count, and subscribes to `sync:notification` (+ `sync:reconnect`)
 * to refetch live. Owner-scoped on the server; every fetch self-gates on the
 * injected `readPermission` (the no-403 invariant — same perm the endpoint
 * enforces).
 *
 * The app binds its concrete REST surface + permission via `deps` (mirroring
 * `createApiClient<TClient>()` — the SDK owns the logic, the app owns the
 * concrete types at the boundary). Returns the `defineStore` handle so the
 * app's thin consumer re-exports `{ Notifications, useNotificationsStore }`
 * and registers it under `Stores.Notifications`.
 */
export function createNotificationsStore({
  api,
  readPermission,
}: NotificationsStoreDeps) {
  return defineStore('Notifications', {
    immer: true,
    state: {
      items: [] as NotificationRow[],
      unread: 0,
      total: 0,
      page: 1,
      perPage: 30,
      unreadOnly: false,
      loading: false,
      error: null as string | null,
    },
    actions: (set, get) => {
      const load = async () => {
        if (!hasPermissionNow(readPermission)) return
        const s = get()
        set(draft => {
          draft.loading = true
          draft.error = null
        })
        try {
          const resp = await api.list({
            page: s.page,
            per_page: s.perPage,
            unread_only: s.unreadOnly,
          })
          set(draft => {
            // Defensive: never let `items` become undefined — a malformed/empty
            // response must not crash the page on `items.length`.
            draft.items = resp.items ?? []
            draft.total = resp.total ?? 0
            draft.unread = resp.unread ?? 0
            draft.loading = false
          })
        } catch (error) {
          set(draft => {
            draft.loading = false
            draft.error =
              error instanceof Error
                ? error.message
                : 'Failed to load notifications'
          })
        }
      }

      const refreshUnread = async () => {
        if (!hasPermissionNow(readPermission)) return
        try {
          const resp = await api.unreadCount()
          set(draft => {
            draft.unread = resp.unread
          })
        } catch {
          /* badge is best-effort */
        }
      }

      /**
       * Fetch a single notification by id, self-gated on the read permission.
       * Used by the toast listener so it never needs the app's ApiClient /
       * permission directly — it reads everything through this store action.
       */
      const fetchOne = async (id: string): Promise<NotificationRow | null> => {
        if (!hasPermissionNow(readPermission)) return null
        try {
          return await api.get({ id })
        } catch {
          return null
        }
      }

      return {
        load,
        refreshUnread,
        fetchOne,
        setPage: (page: number) => {
          set(draft => {
            draft.page = page
          })
          void load()
        },
        setUnreadOnly: (unreadOnly: boolean) => {
          set(draft => {
            draft.unreadOnly = unreadOnly
            draft.page = 1
          })
          void load()
        },
        markRead: async (id: string) => {
          const resp = await api.markRead({ id })
          set(draft => {
            draft.unread = resp.unread
            const row = draft.items.find(n => n.id === id)
            if (row && !row.read_at) row.read_at = new Date().toISOString()
          })
        },
        markAllRead: async () => {
          await api.markAllRead()
          set(draft => {
            draft.unread = 0
            const now = new Date().toISOString()
            for (const n of draft.items) if (!n.read_at) n.read_at = now
          })
        },
        remove: async (id: string) => {
          await api.delete({ id })
          set(draft => {
            draft.items = draft.items.filter(n => n.id !== id)
          })
          void refreshUnread()
        },
        clearError: () =>
          set(draft => {
            draft.error = null
          }),
      }
    },
    init: ({ on, actions }) => {
      const reload = () => {
        if (!hasPermissionNow(readPermission)) return
        void actions.load()
      }
      on('sync:notification', reload)
      on('sync:reconnect', reload)
      reload()
    },
  })
}
