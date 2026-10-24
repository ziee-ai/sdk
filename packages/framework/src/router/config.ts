import type { ReactNode } from 'react'

// @ziee/framework/router — injectable router configuration.
//
// The router component is a lazy, self-mounting component (it reads routes from
// the `Routes` store), so its app-specific knobs are injected through a
// module-level holder — the same DI pattern as the api-client's
// `setBaseUrlResolver`. `createRouterModule(options)` writes them once at
// module-registration time; `RouterComponent` reads them at render.

/** A gate the app supplies to enforce its own permission model on routes that
 *  carry a `permission` value. Returns the children when allowed, or a
 *  replacement (e.g. an inline 403 panel) when denied. */
export type RoutePermissionGate = (args: {
  permission: unknown
  children: ReactNode
}) => ReactNode

/** A route location face (react-router's `Location`, minus the `state`
 *  payload's typing). The seam's getter returns one of these; `RouterComponent`
 *  hands it to `<Routes location={...}>` as the render override. */
export interface RouterLocation {
  pathname: string
  search: string
  hash: string
  /** The router's own per-entry key (the route tree's stable identity). */
  key: string
  /** The history entry's state payload (opaque — the router never reads it). */
  state?: unknown
}

export interface RouterConfig {
  /** Where to redirect unauthenticated access to a protected route when no
   *  `routeGuards` are registered (fail-closed). Default: "/auth". */
  loginPath: string
  /** Where the catch-all "*" route redirects an authenticated user.
   *  Default: "/". */
  homePath: string
  /** Suspense fallback shown while a lazy route chunk loads.
   *  Default: `null`. */
  fallback: ReactNode
  /** Optional app permission gate (see `RoutePermissionGate`). When unset, a
   *  route's `permission` is ignored. */
  permissionGate: RoutePermissionGate | null
  /** Optional RENDER-time location override (the app-supplied location the
   *  router must paint instead of the committed one — e.g. a navigation
   *  transition's held location). Read by `RouterComponent` at render for
   *  `<Routes location={...}>`. The `null` / unset default is byte-identical
   *  to today's render in every app. */
  routesLocation?: () => RouterLocation | null
  /** The change subscription the router uses to re-render when the held
   *  location changes (the reader side of the app's holder — `routesLocation`
   *  alone cannot re-render the router). Called with `onChange`; returns the
   *  unsubscribe. Unset = nothing ever re-renders via this seam. */
  routesLocationSubscribe?: (onChange: () => void) => () => void
}

const config: RouterConfig = {
  loginPath: '/auth',
  homePath: '/',
  fallback: null,
  permissionGate: null,
}

/** Merge partial router config (called once by `createRouterModule`). */
export const setRouterConfig = (partial: Partial<RouterConfig>): void => {
  Object.assign(config, partial)
}

/** Read the current router config (used by `RouterComponent`). */
export const getRouterConfig = (): RouterConfig => config
