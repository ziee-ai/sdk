// @ziee/notification-ui — shared types for the reusable notification UI shell.
//
// This package ships the GENERIC bell + toast + inbox + store. It is
// app-agnostic: it reads the app's notification REST surface through the
// `NotificationApiPort` SEAM (the app injects its generated `ApiClient.Notification`)
// and its live rows through the `Stores.Notifications` typed-view (`NotificationsStoreView`),
// never a concrete app store type — mirroring how `@ziee/shell` reads
// `Stores.AppLayout` through a typed-view cast.

import type { PermissionExpr } from '@ziee/framework/permissions'

/**
 * A notification row as the inbox store holds it. Intentionally a loose,
 * superset-compatible view of an app's generated `Notification` (`payload`
 * untyped, `read_at` nullable-optional, no `user_id`) so the SDK stays
 * app-agnostic. The renderer bridges this to the seam's `AppNotification`
 * (structurally identical) via a single documented cast.
 */
export interface NotificationRow {
  id: string
  /** The contributing module's kind (dispatch key for the renderer registry). */
  kind: string
  title: string
  body: string
  /** TRUE => the toast listener may raise a live toast on arrival. */
  interrupt: boolean
  /** Kind-specific structured data the per-kind renderer reads. */
  payload: unknown
  read_at?: string | null
  created_at: string
}

export interface NotificationListParams {
  page?: number
  per_page?: number
  unread_only?: boolean
}

export interface NotificationListResult {
  items: NotificationRow[]
  total: number
  unread: number
}

export interface UnreadResult {
  unread: number
}

/**
 * The app-data SEAM: the notification REST surface the store drives. The
 * consuming app injects its generated `ApiClient.Notification` (structurally
 * assignable) at `createNotificationsStore` time — the app's thin store
 * consumer casts through this interface, keeping the SDK free of any app type.
 */
export interface NotificationApiPort {
  list(params: NotificationListParams): Promise<NotificationListResult>
  unreadCount(): Promise<UnreadResult>
  get(params: { id: string }): Promise<NotificationRow>
  markRead(params: { id: string }): Promise<UnreadResult>
  markAllRead(): Promise<unknown>
  delete(params: { id: string }): Promise<unknown>
}

/** Dependencies the app binds when creating the inbox store. */
export interface NotificationsStoreDeps {
  /** The app's notification REST surface (inject `ApiClient.Notification`). */
  api: NotificationApiPort
  /**
   * The read permission gating the inbox. MUST equal the perm the list/get
   * endpoints enforce so the store self-gates (the no-403 invariant): a role
   * without the grant fetches nothing rather than 403-looping.
   */
  readPermission: PermissionExpr
}

/**
 * The reactive shape the bell / toast / inbox read off `Stores.Notifications`
 * through a typed-view cast (the app registers the store under this name via
 * the factory). Mirrors `@ziee/shell`'s `Stores.AppLayout` typed-view seam.
 */
export interface NotificationsStoreView {
  items: NotificationRow[]
  unread: number
  total: number
  page: number
  perPage: number
  unreadOnly: boolean
  loading: boolean
  error: string | null
  load: () => void
  refreshUnread: () => void
  fetchOne: (id: string) => Promise<NotificationRow | null>
  setPage: (page: number) => void
  setUnreadOnly: (unreadOnly: boolean) => void
  markRead: (id: string) => void
  markAllRead: () => void
  remove: (id: string) => void
  clearError: () => void
}
