import type { ComponentType, ReactNode } from 'react'
import { BrowserRouter, Routes, Route, Navigate, Outlet } from 'react-router-dom'
import { ModuleSystem } from '../stores'
import { routesSeam } from '../app-seam'
import { LazyRouteRenderer } from './LazyRouteRenderer'
import { LayoutRouteElement, layoutRouteKey } from './layout-route'
import { getRouterConfig } from './config'
import type { LayoutDefinition, RouteConfig } from './types'

// @ziee/framework/router — the router component.
//
// A domain-agnostic port of ziee's `RouterComponent`. Responsibilities:
//  - group routes by auth requirement (protected vs public),
//  - group routes by layout (one layout instance per layout object),
//  - wrap protected routes in the registered `routeGuards` (fail-closed),
//  - apply the app's injected permission gate to routes carrying `permission`,
//  - mount `routerEffects` inside the router context.

let warnedNoGate = false

/**
 * Materialize a route's element via `LazyRouteRenderer`, wrapping it in the
 * app's injected permission gate when the route carries a `permission`.
 */
function renderRouteElement(route: RouteConfig): ReactNode {
  const { fallback, permissionGate } = getRouterConfig()
  const inner = (
    <LazyRouteRenderer
      component={route.element}
      fallback={fallback}
      debugId={route.path}
    />
  )
  if (route.permission === undefined) return inner
  if (!permissionGate) {
    if (!warnedNoGate) {
      warnedNoGate = true
      console.warn(
        '[router] A route declares `permission` but no permissionGate is ' +
          'registered (createRouterModule({ permissionGate })); the permission ' +
          'is ignored and the route renders ungated.',
      )
    }
    return inner
  }
  return permissionGate({ permission: route.permission, children: inner })
}

export function RouterComponent() {
  const { routes } = routesSeam.get() as { routes: RouteConfig[] }
  const { loginPath, homePath, fallback } = getRouterConfig()

  const protectedRoutes = routes.filter((r: RouteConfig) => r.requiresAuth)
  const publicRoutes = routes.filter((r: RouteConfig) => !r.requiresAuth)

  /** Render a list of routes, grouping them by layout object. */
  const renderRoutesForLayoutGroup = (routeList: RouteConfig[]) => {
    const routesByLayout = new Map<
      LayoutDefinition | null,
      RouteConfig[]
    >()

    routeList.forEach(route => {
      const layoutKey = route.layout || null
      if (!routesByLayout.has(layoutKey)) routesByLayout.set(layoutKey, [])
      routesByLayout.get(layoutKey)!.push(route)
    })

    return Array.from(routesByLayout.entries()).map(([layoutDef, group]) => {
      if (!layoutDef) {
        return group.map(route => (
          <Route
            key={route.path}
            path={route.path}
            element={renderRouteElement(route)}
            index={route.index}
          />
        ))
      }

      return (
        <Route
          // Identity-derived, never `component.name`: a lazy layout is an exotic
          // object with no name, so every one of them used to key as 'layout'
          // and collapse into a single group. See `./layout-route.tsx`.
          key={layoutRouteKey(layoutDef)}
          element={
            <LayoutRouteElement layoutDef={layoutDef} fallback={fallback} />
          }
        >
          {group.map(route => (
            <Route
              key={route.path}
              path={route.path}
              element={renderRouteElement(route)}
              index={route.index}
            />
          ))}
        </Route>
      )
    })
  }

  // Effect-only components that must mount inside the router (so they can use
  // useNavigate/useLocation). Each returns null and works in a useEffect.
  const routerEffects = (ModuleSystem.slots.get('routerEffects') ||
    []) as Array<{ id: string; component: ComponentType }>

  // Guards contributed by the app's auth feature. The router owns the slot
  // type and composes them; it imports no guard.
  const guards = (ModuleSystem.slots.get('routeGuards') || []) as Array<{
    id: string
    component: ComponentType<{ children: ReactNode }>
  }>

  if (guards.length === 0 && protectedRoutes.length > 0) {
    console.error(
      '[router] No routeGuards registered; protected routes are sealed. ' +
        'Did the auth module fail to load?',
    )
  }

  // Wrap `inner` in the registered guards (first-registered = outermost). With
  // no guard, seal protected content to the login path (fail-closed).
  const guardProtected = (inner: ReactNode): ReactNode =>
    guards.length > 0 ? (
      guards.reduceRight<ReactNode>(
        (acc, g) => <g.component key={g.id}>{acc}</g.component>,
        inner,
      )
    ) : (
      <Navigate to={loginPath} replace />
    )

  return (
    <BrowserRouter>
      {routerEffects.map(({ id, component: Effect }) => (
        <Effect key={id} />
      ))}
      <Routes>
        {protectedRoutes.length > 0 && (
          <Route element={guardProtected(<Outlet />)}>
            {renderRoutesForLayoutGroup(protectedRoutes)}
          </Route>
        )}

        {renderRoutesForLayoutGroup(publicRoutes)}

        <Route
          path="*"
          element={guardProtected(<Navigate to={homePath} replace />)}
        />
      </Routes>
    </BrowserRouter>
  )
}
