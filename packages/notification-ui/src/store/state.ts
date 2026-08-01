import type { StoreSet } from '@ziee/framework/store-kit'
import type { NotificationNavigate, NotificationRow } from '../types'

/**
 * The notification inbox state. `onNavigate` / `inboxPath` are app-supplied
 * passthrough values (seeded from deps at create time) exposed on the store view
 * so the prop-less bell/inbox widgets read them like any other field.
 */
export const notificationsState = {
  items: [] as NotificationRow[],
  unread: 0,
  total: 0,
  page: 1,
  perPage: 30,
  unreadOnly: false,
  loading: false,
  error: null as string | null,
  onNavigate: undefined as NotificationNavigate | undefined,
  inboxPath: undefined as string | undefined,
}

export type NotificationsState = typeof notificationsState
export type NotificationsSet = StoreSet<NotificationsState>
export type NotificationsGet = () => NotificationsState
