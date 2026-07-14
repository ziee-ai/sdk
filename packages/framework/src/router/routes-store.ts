import { defineStore } from '../store-kit'
import type { StoreProxy } from '../stores'
import type { RouteConfig } from './types'

// @ziee/framework/router — the `Routes` store. Modules' `routes` are collected
// here by the router module's `onModuleRegister` hook; `RouterComponent` reads
// `Stores.Routes.routes` and renders them.

export const Routes = defineStore('Routes', {
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

export const useRoutesStore = Routes.store

// Make `Stores.Routes` typed for apps that opt into the router. Merged onto the
// framework's `RegisteredStores` so `Stores.Routes.routes` type-checks.
declare module '../stores' {
  interface RegisteredStores {
    Routes: StoreProxy<ReturnType<typeof useRoutesStore.getState>>
  }
}
