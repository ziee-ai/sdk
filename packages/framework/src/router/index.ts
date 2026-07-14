// @ziee/framework/router — OPTIONAL routing layer (opt-in subpath export).
//
// NOT re-exported from `@ziee/framework`'s main barrel — an app opts in by
// importing this subpath, keeping react-router out of apps that don't route.
// Ships: a `Routes` store, the `CreateModuleOptions.routes` declaration-merge,
// the `routerEffects` / `routeGuards` slots, and `createRouterModule(options)`
// which mounts a react-router `<BrowserRouter>` with layout + guard grouping.
//
// react-router-dom is a PEER dependency (optional) — install it in your app to
// use this entry.
//
// @example
//   // app CSS/boot:
//   import { createRouterModule } from '@ziee/framework/router'
//   registerModules([createRouterModule({ loginPath: '/login' }), ...features])
//
//   // any feature module declares routes (declaration-merged onto
//   // CreateModuleOptions):
//   createModule({ metadata, routes: [{ path: '/x', element: () => import('./X') }] })

import './types' // side-effect: register the declaration-merges

export { createRouterModule } from './module'
export type { CreateRouterModuleOptions } from './module'
export { RouterComponent } from './RouterComponent'
export { Routes, useRoutesStore } from './routes-store'
export { LazyRouteRenderer } from './LazyRouteRenderer'
export {
  setRouterConfig,
  getRouterConfig,
  type RouterConfig,
  type RoutePermissionGate,
} from './config'
export type { RouteConfig, LayoutDefinition } from './types'
