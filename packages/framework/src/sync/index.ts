import { startSyncClient, stopSyncClient } from './SyncClient'

export { startSyncClient, stopSyncClient } from './SyncClient'

interface AuthLike {
  user?: { id: string } | null
}

/** Minimal slice of a zustand store this module needs (DI'd from App so
 *  `core/sync` doesn't depend on the auth module). `subscribe` is typed
 *  loosely to accept zustand's overloaded signature. */
interface AuthStoreLike {
  getState: () => AuthLike
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  subscribe: (...args: any[]) => () => void
}

let initialized = false

/**
 * Wire the SyncClient lifecycle to auth. Call once at app startup.
 *
 * Starts the stream when a user is present, stops on logout, and
 * restarts on a user switch. Subscribes by user id rather than an
 * `isAuthenticated` boolean: a user switch keeps the boolean true but
 * must re-open the stream under the new identity.
 */
export function initSync(authStore: AuthStoreLike): void {
  if (initialized) return
  initialized = true

  let currentUserId: string | undefined
  const apply = (userId: string | undefined) => {
    if (userId === currentUserId) return
    currentUserId = userId
    stopSyncClient()
    if (userId) startSyncClient()
  }

  apply(authStore.getState().user?.id)
  authStore.subscribe((state: AuthLike) => apply(state.user?.id))
}
