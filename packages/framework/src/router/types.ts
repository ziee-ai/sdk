import type {
  ComponentType,
  ReactNode,
  ReactElement,
  LazyExoticComponent,
} from 'react'

// @ziee/framework/router — types.
//
// A domain-agnostic port of ziee's own `ui/src/modules/router` module. It ships
// the same three things ziee's does — a `Routes` store, the
// `CreateModuleOptions.routes` declaration-merge, and the `routerEffects` /
// `routeGuards` slots — but with ZERO app coupling: no `@/core/permissions`,
// no `@ziee/kit`, no `@/core/components`. Permission gating and the loading
// fallback are injected by the consuming app (see `createRouterModule`).

/**
 * A layout component that wraps a group of routes. Routes sharing the same
 * layout object are rendered under one layout instance (a react-router nested
 * route with an `<Outlet/>`).
 */
export interface LayoutDefinition {
  /** The layout component that wraps route content via `children`. */
  component: ComponentType<{ children: ReactNode }>
}

/**
 * A single route. `element` may be an already-rendered element, a
 * `React.lazy` component, or a bare dynamic-import loader
 * (`() => import('./Page')`) — all three are materialized by the router.
 */
export interface RouteConfig {
  /**
   * Route path (e.g. "/chat", "/settings"). Supports react-router dynamic
   * segments (":param") and optional segments (":param?"),
   * e.g. "/chat/:conversationId".
   */
  path: string

  /** Route element — a React element, a lazy component, or a dynamic-import
   *  loader returning `{ default: Component }`. */
  element:
    | ReactElement
    | LazyExoticComponent<ComponentType<any>>
    | (() => Promise<{ default: ComponentType<any> }>)

  /** Whether this route sits behind the registered `routeGuards`
   *  (default: false → public). */
  requiresAuth?: boolean

  /**
   * Optional app-defined permission value. The router does not interpret it —
   * it hands it to the app's injected permission gate (see
   * `createRouterModule({ permissionGate })`). Left generic (`unknown`) on
   * purpose so the router stays domain-agnostic; the app narrows it in its own
   * gate. When no gate is registered, `permission` is ignored (with a one-time
   * warning).
   */
  permission?: unknown

  /** Whether this is an index route. */
  index?: boolean

  /** Layout to wrap this route with (optional). */
  layout?: LayoutDefinition
}

/**
 * Extend `CreateModuleOptions` so any module can declare `routes`. This is how
 * the router collects routes from every registered module (via
 * `onModuleRegister`). Mirrors ziee's own router.
 */
declare module '../module' {
  interface CreateModuleOptions {
    routes?: RouteConfig[]
  }
}

/**
 * The router's two extension slots, owned here (the CONSUMER of the slot owns
 * its type — plugins only fill it):
 *
 *  - `routerEffects` — headless, effect-only components the router mounts
 *    INSIDE the router context so they can use `useNavigate`/`useLocation`.
 *    Each renders `null` and does its work in a `useEffect`.
 *  - `routeGuards` — components wrapping every `requiresAuth` route. The
 *    FIRST-registered guard is the OUTERMOST wrapper. An empty slot is sealed
 *    fail-closed (protected routes redirect to the login path) so protected
 *    content is never rendered ungated.
 */
declare module '../module-system/types' {
  interface Slots {
    routerEffects: Array<{ id: string; component: ComponentType }>
    routeGuards: Array<{
      id: string
      component: ComponentType<{ children: ReactNode }>
    }>
  }
}

export {}
