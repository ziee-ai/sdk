import { routesSeam } from '../app-seam'
import { defineStore } from '../store-kit'
import { registerLazyStore, type StoreProxy } from '../stores'
import type { RouteConfig } from './types'

// @ziee/framework/router — the `Routes` store. Modules' `routes` are collected
// here by the router module's `onModuleRegister` hook; `RouterComponent` reads
// the routes back through `routesSeam` and renders them.
//
// SEAM OWNERSHIP. Retiring the global `Stores` proxy replaced `Stores.Routes`
// with `routesSeam`, but nothing in `packages/` ever called `routesSeam.set()` —
// while BOTH `RouterComponent` and `usePrefetchModules` call `.get()`, which
// throws when uninjected. An app with its OWN router module does not notice,
// because it injects the seam from its own routes-store; an app using
// `createRouterModule` — the router this package ships — boot-crashes on first
// render. The injection therefore belongs HERE, beside the store the seam names,
// so the SDK router is self-sufficient rather than requiring an app to know it
// must inject a seam for a store it never declared.
//
// `registerLazyStore` (not a bare `createStoreProxy`) is what makes this safe
// alongside the `stores: [{ name: 'Routes', store: useRoutesStore }]` entry in
// `module.tsx`: it self-registers under the same name, and `module-system/store`
// tracks it in `selfOwnedStores` so the module registration does not create a
// SECOND proxy with an independent ref count — which would run `init` (and every
// `sync:*` listener) twice.

const RoutesDef = defineStore('Routes', {
  state: {
    routes: [] as RouteConfig[],
  },
  actions: set => ({
    addRoutes: (routes: RouteConfig[]) => {
      set(state => ({ routes: [...state.routes, ...routes] }))
    },
    /** Reset — primarily for tests / hot-reload. */
    resetRoutes: () => {
      set(() => ({ routes: [] as RouteConfig[] }))
    },
  }),
})

export const useRoutesStore = RoutesDef.store
export const Routes = registerLazyStore(RoutesDef)

// Inject the seam the SDK's own router + prefetch read from. Module scope, so it
// runs on import of this file — which `module.tsx` already does.
routesSeam.set(Routes as unknown as { routes: unknown[] })

// Make `Stores.Routes` typed for apps that opt into the router. Merged onto the
// framework's `RegisteredStores` so `Stores.Routes.routes` type-checks.
declare module '../stores' {
  interface RegisteredStores {
    Routes: StoreProxy<ReturnType<typeof useRoutesStore.getState>>
  }
}
