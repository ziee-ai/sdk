import { hasPermissionNow } from '@ziee/framework/permissions'
import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, get: NotificationsGet) => async () => {
  const { api, readPermission } = notificationDeps()
  if (!hasPermissionNow(readPermission)) return
  const s = get()
  // NO in-flight guard here, deliberately. A bare `if (s.loading) return` looks
  // like the sibling list actions' guard but is WRONG for this action: it reads
  // `page` / `perPage` / `unreadOnly` from state, and `setPage` / `setUnreadOnly`
  // mutate those and then call `load()`. A bare drop would discard the new
  // page/filter (leaving the UI showing the new selection with the old items),
  // and would equally discard the `sync:notification` / `sync:reconnect` reload
  // wired in `../index.ts` — silently breaking notify-and-refetch, which is a
  // worse defect than the duplicate request it saves.
  //
  // The duplicate that motivated a guard here (the bell widget, the toast
  // listener and the inbox page each triggering an IDENTICAL
  // `GET /api/notifications` on the same boot) is removed at the transport
  // instead (`@ziee/framework/api-client/inflight`), which coalesces only
  // requests that are literally identical AND concurrent — so a differing-intent
  // reload still gets its own round-trip.
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
