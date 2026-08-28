// @ziee/framework — domain-agnostic UI runtime barrel.
//
// The core meta-framework moved out of ziee's `ui/src/core/*`: the module
// system, the `createModule` authoring surface, the EventBus, the Stores proxy
// + store-kit, and the desktop UI-override seam. The realtime sync client and
// the ApiClient transport runtime live under the `@ziee/framework/sync` and
// `@ziee/framework/api-client` subpaths respectively.
//
// The augmentable declaration-merge interfaces apps extend — `RegisteredStores`
// (`@ziee/framework/stores`), `AppEvents` (`@ziee/framework/events`), `Slots`
// (`@ziee/framework/module-system/types`), `CreateModuleOptions`
// (`@ziee/framework/module`), and `UIOverrides` (`@ziee/framework/overrides`) —
// are re-exported here and augmented per-app via `declare module` against those
// stable subpath specifiers.
export * from './module-system'
export * from './module'
export * from './stores'
export * from './events'
export * from './overrides'
export { createAppStoreSeam, routesSeam } from './app-seam'

// Component-like classification + the one renderer behind every "render
// whatever a module put in this field" surface (routes, slots, banners).
// `markLazyLoader` is the explicit opt-in for a dynamic-import loader whose
// shape cannot be recognised on its own.
export {
  classifyComponentLike,
  isMarkedLazyLoader,
  markLazyLoader,
  type ComponentKind,
  type Classification,
  type LazyLoader,
} from './lazy-component'
export {
  RenderComponentLike,
  type ComponentLike,
  type RenderComponentLikeProps,
} from './lazy-component-view'
