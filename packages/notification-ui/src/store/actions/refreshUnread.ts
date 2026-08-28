import { hasPermissionNow } from '@ziee/framework/permissions'
import { notificationDeps } from '../_deps'
import type { NotificationsGet, NotificationsSet } from '../state'

export default (set: NotificationsSet, _get: NotificationsGet) => async () => {
  const { api, readPermission } = notificationDeps()
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
