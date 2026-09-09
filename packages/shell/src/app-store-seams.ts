import { create } from 'zustand'
import { createAppStoreSeam, createStoreProxy } from '@ziee/framework'

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

/**
 * The AppLayout slice the shell's OPTIONAL chrome reads during render.
 *
 * `setHeaderHidden` is an ACTION: on a real store proxy an action read takes the
 * hook-FREE path, so the placeholder must carry a function here too — a
 * `undefined` would take the reactive path instead and reintroduce the very
 * asymmetry this file exists to remove.
 */
export interface AppLayoutChromeView {
  nativeScroll: boolean
  isSidebarCollapsed: boolean
  setHeaderHidden: (hidden: boolean) => void
}

/**
 * Stand-in AppLayout store used while the app's `app-layout` module chunk has
 * not been imported yet (`appLayoutSeam.set(AppLayout)` is a module side effect
 * of a LAZY chunk, so `peek()` is null for an unpredictable number of renders,
 * including in the dev gallery and on layout-less routes).
 *
 * WHY A REAL STORE PROXY AND NOT `?? { nativeScroll: false }`:
 *
 * A reactive store-proxy field read IS a hook in this codebase (path 4 of
 * `createStoreProxy` calls `useEffect` + `useStore`). So
 *
 *     (appLayoutSeam.peek() as V | null)?.nativeScroll ?? false      // WRONG
 *     ((appLayoutSeam.peek() as V | null) ?? { nativeScroll: false }).nativeScroll  // ALSO WRONG
 *
 * both make the component's hook COUNT a function of whether the seam has been
 * injected: zero hooks before, two after. When the chunk lands mid-life React
 * throws "Rendered more hooks than during the previous render" and the whole
 * settings subtree hits the error boundary. (Observed on the gallery's
 * `seeded-file-rag-error` surface on 3 of 16 loads; the hook table names
 * `SettingsPageContainer`, `1. useId → useId`, `2. undefined → useEffect`.)
 *
 * Routing BOTH states through a store proxy makes the read take the identical
 * path either way, so the count cannot vary. That is a structural property of
 * the value returned here, not a guard some future call site can forget.
 */
const placeholderAppLayout = createStoreProxy(
  create<AppLayoutChromeView>(() => ({
    nativeScroll: false,
    isSidebarCollapsed: false,
    setHeaderHidden: () => {},
  })) as never,
) as unknown as AppLayoutChromeView

/**
 * The ONLY way the shell's optional chrome may read AppLayout during render.
 * Never returns null, so every consumer's read is unconditional and its hook
 * count is stable across seam injection.
 */
export function appLayoutChrome(): AppLayoutChromeView {
  return (appLayoutSeam.peek() as AppLayoutChromeView | null) ?? placeholderAppLayout
}
