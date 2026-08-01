import { createAppStoreSeam } from '@ziee/framework'

import type { NotificationsStoreView } from './types'

/** SEAM: the app injects its Notifications store once at boot via `notificationsSeam.set(Notifications)`. */
export const notificationsSeam = createAppStoreSeam<NotificationsStoreView>('Notifications')

/**
 * Read the app-registered `Stores.Notifications` through a typed-view cast —
 * the SDK never depends on the app's concrete store type (mirrors how
 * `@ziee/shell` reads `Stores.AppLayout`). The app registers the store (created
 * by `createNotificationsStore`) under the `Notifications` name, so at runtime
 * this resolves the same reactive proxy the app would read directly.
 *
 * IMPORTANT (store-kit reactivity): the returned object IS the reactive proxy —
 * destructuring a field off it during render subscribes the component. Read it
 * unconditionally at the top of a component body, never behind a branch.
 */
export function notificationsStore(): NotificationsStoreView {
  return notificationsSeam.get()
}
