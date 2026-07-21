import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, _get: NotificationsGet) =>
  async (id: string) => {
    const { api } = notificationDeps()
    const resp = await api.markRead({ id })
    set(draft => {
      draft.unread = resp.unread
      const row = draft.items.find(n => n.id === id)
      if (row && !row.read_at) row.read_at = new Date().toISOString()
    })
  }
