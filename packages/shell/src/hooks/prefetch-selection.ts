import { classifyComponentLike } from '@ziee/framework'

/**
 * WHICH route chunks the idle prefetcher is allowed to warm.
 *
 * Split out of `usePrefetchModules` because this is the whole policy and it was
 * previously unreachable by any test: it lived inside an effect, behind
 * `requestIdleCallback`, behind a store read. As a pure function it is asserted
 * directly — see `prefetch-selection.test.ts`.
 */

/** The minimal route shape the prefetcher reads off the app-registered `Routes`
 *  store. `path` / `requiresAuth` / `permission` are what scope the selection to
 *  routes the current visitor can actually reach. */
export interface PrefetchRoute {
  element: unknown
  path?: string
  requiresAuth?: boolean
  permission?: unknown
}

export interface PrefetchSelectionInput {
  routes: readonly PrefetchRoute[]
  /** Is a user session present? */
  isAuthed: boolean
  /** Current location, so the route already loading is not re-requested. */
  pathname: string
  /** The same permission check the route guard enforces, so prefetch and
   *  navigability agree. Only consulted for a SIGNED-IN visitor. */
  hasPermission: (permission: unknown) => boolean
}

/**
 * Turn a route path pattern (with `:param` / `:param?` segments) into a regex
 * so we can skip prefetching the CURRENT route (its chunk is already loading).
 * Best-effort only — a miss just re-triggers a cached loader (a no-op).
 */
export function pathMatchesCurrent(pattern: string, pathname: string): boolean {
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
 * The routes whose chunks may be warmed right now.
 *
 * ── Why a signed-out visitor is NOT excluded outright ───────────────────────
 * This used to begin `if (!isAuthed) return []` — a logged-out visitor got no
 * prefetch at all. That was written for an authed-only tool, where the only
 * signed-out surface is a login page and the reasoning ("a logged-out user can
 * reach no protected route, keep the login page lean") is sound. It is wrong
 * for a product with a real public surface — a reading site, a docs site, a
 * storefront — where the SIGNED-OUT path is the majority of traffic and the
 * public routes it can reach are exactly the ones worth warming. COMIZY hit
 * this: its signed-out home, browse, series and reader routes are public, and
 * every one of them paid a cold chunk download on navigation.
 *
 * The original concern is kept, and is in fact the rule that replaces the blanket
 * exclusion: a visitor is never handed a chunk they cannot route to.
 *   - `requiresAuth` routes are skipped for a guest — those are precisely the
 *     ones the router's `routeGuards` bounce to the login path.
 *   - a route carrying a `permission` is skipped for a guest without consulting
 *     the permission check at all. A guest satisfies no permission, so the
 *     answer is fixed; skipping early also means the prefetcher never reaches
 *     into the auth store before a session exists.
 *   - for a signed-in visitor the behaviour is unchanged: the route's
 *     `permission` is put through the same check the guard uses.
 */
export function selectPrefetchRoutes({
  routes,
  isAuthed,
  pathname,
  hasPermission,
}: PrefetchSelectionInput): PrefetchRoute[] {
  return routes.filter(route => {
    // Only dynamic-import loaders are prefetchable; an already-built element, a
    // `React.lazy` exotic and a `React.memo` exotic are not functions and have
    // nothing to request.
    if (typeof route.element !== 'function') return false

    // …and not every FUNCTION is a loader. Warming a route means CALLING its
    // element, so a plain component here is invoked outside a render and throws
    // React's *Invalid hook call* from inside an idle callback. The renderers
    // already have to tell these apart (`@ziee/framework/lazy-component`), so
    // ask them rather than guessing a second time. A props-taking function and a
    // class component are settled NOT-loaders and are skipped outright; the one
    // residual shape — a named 0-arg function with no `import(` in its source,
    // which is a loader and a props-less component alike — is still attempted,
    // because skipping it would silently disable prefetch for a real loader
    // built by a helper. The caller invokes it defensively.
    const { kind, ambiguous } = classifyComponentLike(route.element)
    if (kind !== 'loader' && !(kind === 'component' && ambiguous)) return false

    if (!isAuthed) {
      // Behind the router's auth guard — unreachable, so not worth a request.
      if (route.requiresAuth) return false
      // A guest holds no permissions; a gated route is unreachable by
      // definition.
      if (route.permission !== undefined) return false
    } else if (
      route.permission !== undefined &&
      !hasPermission(route.permission)
    ) {
      return false
    }

    // The current route's chunk is already loading.
    if (route.path && pathMatchesCurrent(route.path, pathname)) return false

    return true
  })
}
