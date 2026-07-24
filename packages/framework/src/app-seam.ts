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
  peek: () => T | null
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
    // Non-throwing read for consumers that legitimately render in a store-LESS
    // context (an isolated overlay in the dev gallery, or a layout-less route)
    // and degrade gracefully when the seam is absent. `get()` stays the loud
    // boot-critical read (the router's Routes seam MUST be present); `peek()` is
    // the opt-in "the store may not be here yet, and that's fine" read that the
    // optional shell chrome (DivScrollY/SettingsPageContainer/HeaderBar) uses.
    peek: (): T | null => injected,
  }
}

/** SEAM: the app's `Routes` store (`{ routes }`). Injected by the app's router module. */
export const routesSeam = createAppStoreSeam<{ routes: unknown[] }>('Routes')
