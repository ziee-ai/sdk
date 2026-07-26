import { hasPermissionNow } from '@ziee/framework/permissions'
import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, get: NotificationsGet) => async () => {
  const { api, readPermission } = notificationDeps()
  if (!hasPermissionNow(readPermission)) return
  const s = get()
  // In-flight guard — mirrors every sibling list action (e.g. chatHistory's
  // `loadRecentConversations`). Without it, the store's several consumers (the
  // bell widget, the toast listener, the inbox page) each triggered their own
  // `GET /api/notifications` on the same boot.
  if (s.loading) return
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
        error instanceof Error ? error.message : 'Failed to load notifications'
    })
  }
}
