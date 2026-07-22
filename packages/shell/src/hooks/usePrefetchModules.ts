import { useEffect, isValidElement } from 'react'
import { routesSeam } from '@ziee/framework'
import { authStoreProxy, hasPermissionNow } from '@ziee/framework/permissions'
import type { PermissionExpr } from '@ziee/framework/permissions'

/** The minimal route shape this hook reads off the app-registered `Routes`
 *  store (see the SEAM note below). `path`/`requiresAuth`/`permission` are read
 *  so prefetch can be scoped to routes the current user can actually reach. */
interface PrefetchRoute {
  element: unknown
  path?: string
  requiresAuth?: boolean
  permission?: PermissionExpr
}

/** Turn a route path pattern (with `:param` / `:param?` segments) into a regex
 *  so we can skip prefetching the CURRENT route (its chunk is already loading).
 *  Best-effort only — a miss just re-triggers a cached loader (a no-op). */
function pathMatchesCurrent(pattern: string, pathname: string): boolean {
  const rx = new RegExp(
    '^' +
      pattern
        .replace(/\/:[^/?]+\?/g, '(?:/[^/]+)?') // optional segment
        .replace(/:[^/]+/g, '[^/]+') // required param
        .replace(/\//g, '\\/') +
      '\\/?$',
  )
  return rx.test(pathname)
}

/**
 * Hook to prefetch lazy-loaded route chunks after initial render, on browser
 * idle — so that once the user is signed in, navigating to another page loads
 * instantly instead of paying a chunk download.
 *
 * SCOPED prefetch (this is the important part): the app registers ~50 routes,
 * many gated behind permissions or auth. Prefetching ALL of them was actively
 * harmful — the unauthenticated login page downloaded every settings/admin/chat
 * chunk it can never navigate to, competing for bandwidth during boot. So:
 *
 *   1. Unauthenticated (no `Auth.user`) → prefetch NOTHING. A logged-out user
 *      can reach no protected route; the login page should stay lean.
 *   2. Authenticated → prefetch only routes whose `permission` the user
 *      satisfies (via the same `hasPermissionNow` the route guards use, so
 *      prefetch and navigability agree), skipping the CURRENT route.
 *   3. No forced `requestIdleCallback` timeout — run at TRUE idle so prefetch
 *      never preempts the boot data fetches.
 *
 * SEAM: reads the app-registered `Routes` store (populated by
 * `@ziee/framework/router` or an app-local router module) through a typed view,
 * so the shell doesn't depend on the router being present at type-check time.
 */
export function usePrefetchModules() {
  const { routes } = (
    { Routes: routesSeam.get() as { routes: PrefetchRoute[] } }
  ).Routes

  // Reactive read: re-run the effect once the user signs in (null → set), so
  // prefetch kicks off only after auth (and its permissions) are available.
  // `user` and `permissions` arrive together from /auth/me, so user-presence is
  // a sufficient "authenticated + permissions ready" signal.
  const isAuthed = authStoreProxy().user != null

  useEffect(() => {
    // Gate 0: the `VITE_STORE_PREFETCH=off` compile-time flag disables ALL
    // prefetch (store-action warm AND this route-chunk prefetch), so a build can
    // be measured with zero idle-prefetch warming. Vite inlines the env var, so
    // this const-folds away in an `=off` build.
    if (import.meta.env.VITE_STORE_PREFETCH === 'off') return
    // Gate 1: no signed-in user → prefetch nothing.
    if (!isAuthed) return

    const prefetch = () => {
      const pathname =
        typeof window !== 'undefined' ? window.location.pathname : ''
      routes.forEach(route => {
        const loader = route.element
        // Only lazy loader functions are prefetchable (not eager elements).
        if (typeof loader !== 'function' || isValidElement(loader)) return
        // Gate 2: skip routes the user is not permitted to reach — the same
        // check the route guard enforces, so we never prefetch an unreachable
        // chunk.
        if (route.permission && !hasPermissionNow(route.permission)) return
        // Skip the current route — its chunk is already loading.
        if (route.path && pathMatchesCurrent(route.path, pathname)) return
        ;(loader as () => Promise<{ default: React.ComponentType<any> }>)()
      })
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
