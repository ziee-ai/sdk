/**
 * Injection seam for an app-registered store the SDK must read but cannot import
 * (the app depends on the SDK, never the reverse). This REPLACES the former
 * global `Stores.<Name>` lookup: the consuming app calls `set(<Name>)` once at
 * boot, and SDK code calls `get()` at render. Reactivity is byte-identical to the
 * old `Stores.<Name>` — `get()` returns the very same reactive store proxy the
 * app holds, so destructuring a field off it during render still subscribes.
 *
 * Why not a global registry: the old `Stores` proxy forced every store to be
 * registered eagerly (so `Stores.X` could resolve any of them), dragging every
 * store shell into the entry chunk — O(all-stores) boot cost. A handful of typed
 * seams for the few boot-critical app stores the SDK genuinely needs (AppLayout,
 * Routes, ConfigClient, Notifications, Auth) keeps the SDK app-agnostic with zero
 * global registry and O(page-stores) boot.
 */
export function createAppStoreSeam<T>(name: string): {
  set: (view: T) => void
  get: () => T
} {
  let injected: T | null = null
  return {
    set: (view: T) => {
      injected = view
    },
    get: (): T => {
      if (injected == null) {
        throw new Error(
          `[app-seam] the "${name}" store was not registered — the app must inject it (set it) at boot`,
        )
      }
      return injected
    },
  }
}

/** SEAM: the app's `Routes` store (`{ routes }`). Injected by the app's router module. */
export const routesSeam = createAppStoreSeam<{ routes: unknown[] }>('Routes')
