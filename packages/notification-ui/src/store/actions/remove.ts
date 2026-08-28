import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'
import refreshUnreadFactory from './refreshUnread'

export default (set: NotificationsSet, get: NotificationsGet) => {
  const refreshUnread = refreshUnreadFactory(set, get)
  return async (id: string) => {
    const { api } = notificationDeps()
    await api.delete({ id })
    set(draft => {
      draft.items = draft.items.filter(n => n.id !== id)
    })
    void refreshUnread()
  }
}
