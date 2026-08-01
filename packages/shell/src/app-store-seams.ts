import { createAppStoreSeam } from '@ziee/framework'

/**
 * SEAMS: the app-registered stores the shell reads but cannot import (the app
 * depends on the shell, not the reverse). Replaces the former `Stores.AppLayout`
 * / `Stores.ConfigClient` global lookups. The consuming app injects each once at
 * boot via `appLayoutSeam.set(AppLayout)` / `configClientSeam.set(ConfigClient)`.
 *
 * Typed loosely (`unknown`) — each consumer casts to its own local view
 * interface, exactly as it previously cast `Stores as unknown as { AppLayout }`.
 */
export const appLayoutSeam = createAppStoreSeam<unknown>('AppLayout')
export const configClientSeam = createAppStoreSeam<unknown>('ConfigClient')
