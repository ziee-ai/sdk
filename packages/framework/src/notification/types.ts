// SDK notification feature — shared FE types. The durable inbox row shape
// (mirrors the `ziee-notification` crate's `Notification`), plus the per-module
// renderer-registry types. This is the frontend half of the per-module
// notification contribution seam: a module registers a `kind` + how to render
// it; the inbox dispatches on `kind`.

/** A durable notification row as delivered to the client. */
export interface AppNotification {
  id: string
  /** The contributing module's kind (dispatch key), e.g. `study_share_invite`. */
  kind: string
  title: string
  body: string
  interrupt: boolean
  /** Kind-specific structured data the renderer reads. */
  payload: Record<string, unknown>
  read_at: string | null
  created_at: string
}

/** Actions a renderer can invoke on its notification. */
export interface NotificationRendererCtx {
  /** Mark this notification read. */
  markRead: (id: string) => void
  /** Delete this notification. */
  remove: (id: string) => void
  /** Close the inbox popover (e.g. after an action navigates away). */
  close: () => void
}
