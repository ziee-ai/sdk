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
