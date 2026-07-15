// The per-module notification-kind RENDERER registry (frontend half of the SDK
// notification seam). A module registers a renderer for its `kind`; the inbox
// dispatches on `notification.kind`. Unknown kinds fall back to a generic
// title/body render in the inbox shell.

import type { ReactNode } from 'react'
import type { AppNotification, NotificationRendererCtx } from './types'

export interface NotificationKindRenderer {
  /** Main content for a notification of this kind (defaults to title/body if
   *  this kind is unregistered). */
  render: (n: AppNotification, ctx: NotificationRendererCtx) => ReactNode
  /** Optional inline action row (e.g. Accept / Decline for an invite). */
  actions?: (n: AppNotification, ctx: NotificationRendererCtx) => ReactNode
}

const registry = new Map<string, NotificationKindRenderer>()

/** Register how a notification `kind` renders. Call once at module load. */
export function registerNotificationKind(
  kind: string,
  renderer: NotificationKindRenderer,
): void {
  registry.set(kind, renderer)
}

/** The renderer for `kind`, or `undefined` (the inbox uses a generic fallback). */
export function getNotificationRenderer(
  kind: string,
): NotificationKindRenderer | undefined {
  return registry.get(kind)
}

/** All registered kinds (introspection / tests). */
export function registeredNotificationKinds(): string[] {
  return [...registry.keys()]
}
