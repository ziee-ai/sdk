// @ziee/notification-ui — the reusable notification UI SHELL.
//
// Ships the GENERIC, app-agnostic notification surfaces any SDK-consuming app
// mounts as-is: the durable inbox store, the sidebar bell + unread badge, the
// arrival toast listener, and the full inbox page. It reads the app's
// notification REST surface through the `NotificationApiPort` SEAM (the app
// injects its generated `ApiClient.Notification` at `createNotificationsStore`
// time) and its live rows through the `Stores.Notifications` typed-view — never
// a concrete app type.
//
// EXTENSIBILITY: per-kind inbox rendering is the app's job — it registers
// renderers via the `@ziee/framework/notification` registry (the bell/inbox
// dispatch on `notification.kind`, falling back to a generic title/body block).
// NAVIGATION: the app supplies an `onNavigate(n, navigate)` seam + `inboxPath`
// at `createNotificationsStore` time — the SDK hardcodes ZERO app routes.
//
// LAYERING: deps are `@ziee/kit` + `@ziee/framework` (+ `@ziee/shell` for the
// shared `SettingsPageContainer` scaffold). `@ziee/framework` never depends on
// `@ziee/kit`; this package is where the kit-having notification UI lives.

export { createNotificationsStore } from './store'
export { NotificationBellWidget } from './NotificationBellWidget'
export { NotificationItem } from './NotificationItem'
export { NotificationToastListener } from './NotificationToastListener'
export { NotificationsPage } from './NotificationsPage'

export type {
  NotificationApiPort,
  NotificationListParams,
  NotificationListResult,
  NotificationNavigate,
  NotificationRow,
  NotificationsStoreDeps,
  NotificationsStoreView,
  UnreadResult,
} from './types'
export { notificationsSeam, notificationsStore } from './storeView'
