import { useEffect } from 'react'

import { useEventBusStore } from '@ziee/framework/events'
import { message } from '@ziee/kit'

import { notificationsStore } from './storeView'

/**
 * Globally-mounted (route-independent) listener that raises a live toast when a
 * new notification arrives — but only when its `interrupt` flag is set (a
 * silent task's result lands in the inbox without a toast). Generic +
 * app-agnostic: it reads everything through the app-registered
 * `Stores.Notifications` store (its `fetchOne` action self-gates on the read
 * permission) and the framework EventBus seam — never the app's ApiClient /
 * permission directly. The durable row + badge update are handled by the store's
 * own `sync:notification` subscription.
 *
 * Toast severity is derived generically from the notification `kind`: a kind
 * ending in `_failed` / `_error` shows an error toast, everything else an info
 * toast. An app registers richer per-kind rendering via the
 * `@ziee/framework/notification` renderer registry (inbox), independent of this
 * transient toast.
 */
export function NotificationToastListener() {
  useEffect(() => {
    const GROUP = 'NotificationToastListener'
    const bus = useEventBusStore.getState()
    bus.on(
      'sync:notification',
      async event => {
        if (event.data.action !== 'create') return
        const id = event.data.id
        // Nil id = a bulk "list changed" signal (read-all / prune), not a new row.
        if (!id || id === '00000000-0000-0000-0000-000000000000') return
        // fetchOne self-gates on the read permission and returns null on error.
        const n = await notificationsStore().fetchOne(id)
        if (!n || !n.interrupt) return
        if (n.kind.endsWith('_failed') || n.kind.endsWith('_error')) {
          message.error(n.title, { description: n.body || undefined })
        } else {
          message.info(n.title, { description: n.body || undefined })
        }
      },
      GROUP,
    )
    return () => {
      useEventBusStore.getState().removeGroupListeners(GROUP)
    }
  }, [])
  return null
}
