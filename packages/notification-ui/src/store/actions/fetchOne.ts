import { hasPermissionNow } from '@ziee/framework/permissions'
import { notificationDeps } from '../_deps'
import type { NotificationRow } from '../../types'
import type { NotificationsGet, NotificationsSet } from '../state'

/**
 * Fetch a single notification by id, self-gated on the read permission. Used by
 * the toast listener so it never needs the app's ApiClient / permission directly.
 */
export default (_set: NotificationsSet, _get: NotificationsGet) =>
  async (id: string): Promise<NotificationRow | null> => {
    const { api, readPermission } = notificationDeps()
    if (!hasPermissionNow(readPermission)) return null
    try {
      return await api.get({ id })
    } catch {
      return null
    }
  }
