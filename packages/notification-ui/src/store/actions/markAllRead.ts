import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, _get: NotificationsGet) => async () => {
  const { api } = notificationDeps()
  await api.markAllRead()
  set(draft => {
    draft.unread = 0
    const now = new Date().toISOString()
    for (const n of draft.items) if (!n.read_at) n.read_at = now
  })
}
