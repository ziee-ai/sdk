import type { StoreProxy } from '../stores'
import type { useModuleSystemStore } from './store'

declare module '../stores' {
  interface RegisteredStores {
    ModuleSystem: StoreProxy<ReturnType<typeof useModuleSystemStore.getState>>
  }
}

export {}
