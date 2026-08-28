import { useEffect } from 'react'
import { routesSeam } from '@ziee/framework'
import { authStoreProxy, hasPermissionNow } from '@ziee/framework/permissions'
import type { PermissionExpr } from '@ziee/framework/permissions'
import { selectPrefetchRoutes, type PrefetchRoute } from './prefetch-selection'

/**
 * Hook to prefetch lazy-loaded route chunks after initial render, on browser
 * idle — so navigating to another page loads instantly instead of paying a
 * chunk download.
 *
 * SCOPED prefetch (this is the important part): an app registers dozens of
 * routes, many gated behind auth or a permission. Prefetching ALL of them is
 * actively harmful — it downloads chunks the visitor can never navigate to,
 * competing for bandwidth during boot. Which routes survive that scoping is
 * `selectPrefetchRoutes` in `./prefetch-selection`, where the policy is stated
 * and unit-tested; the notable part is that a SIGNED-OUT visitor is scoped, not
 * excluded (it used to be excluded outright — see that file).
 *
 * Gate 3 lives here: no forced `requestIdleCallback` timeout, so prefetch runs
 * at TRUE idle and never preempts the boot data fetches.
 *
 * SEAM: reads the app-registered `Routes` store (populated by
 * `@ziee/framework/router` or an app-local router module) through a typed view,
 * so the shell doesn't depend on the router being present at type-check time.
 */
export function usePrefetchModules() {
  const { routes } = (
    { Routes: routesSeam.get() as { routes: PrefetchRoute[] } }
  ).Routes

  // Reactive read: re-run the effect when the session appears or disappears, so
  // the eligible set is recomputed. `user` and `permissions` arrive together
  // from /auth/me, so user-presence is a sufficient "authenticated +
  // permissions ready" signal.
  const isAuthed = authStoreProxy().user != null

  useEffect(() => {
    // Gate 0: the `VITE_STORE_PREFETCH=off` compile-time flag disables ALL
    // prefetch (store-action warm AND this route-chunk prefetch), so a build can
    // be measured with zero idle-prefetch warming. Vite inlines the env var, so
    // this const-folds away in an `=off` build.
    if (import.meta.env.VITE_STORE_PREFETCH === 'off') return

    const prefetch = () => {
      const selected = selectPrefetchRoutes({
        routes,
        isAuthed,
        pathname: typeof window !== 'undefined' ? window.location.pathname : '',
        hasPermission: permission =>
          hasPermissionNow(permission as PermissionExpr),
      })
      for (const route of selected) {
        // Defensive: `selectPrefetchRoutes` still admits the one shape it cannot
        // tell apart from a props-less component. Calling one of those invokes a
        // component outside a render, and an uncaught throw here would surface
        // as an unhandled error from an idle callback with nothing pointing at
        // the prefetcher.
        try {
          const started = (route.element as () => unknown)()
          if (
            started !== null &&
            typeof started === 'object' &&
            typeof (started as { then?: unknown; catch?: unknown }).catch ===
              'function'
          ) {
            // A rejected chunk load is the network's problem, not a page error;
            // navigation will retry it and surface it in a boundary that can.
            ;(started as Promise<unknown>).catch(() => {})
          }
        } catch {
          // Prefetch is best-effort by definition.
        }
      }
    }

    // Gate 3: true idle, no forced timeout (don't preempt boot fetches).
    if ('requestIdleCallback' in window) {
      const handle = requestIdleCallback(prefetch)
      return () => cancelIdleCallback(handle)
    } else {
      // Fallback for browsers without requestIdleCallback (Safari < 16).
      const timer = setTimeout(prefetch, 1000)
      return () => clearTimeout(timer)
    }
  }, [routes, isAuthed])
}
