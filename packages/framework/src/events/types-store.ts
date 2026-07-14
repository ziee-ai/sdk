import type { StoreProxy } from '../stores'
import type { useEventBusStore } from './store'

declare module '../stores' {
  interface RegisteredStores {
    EventBus: StoreProxy<ReturnType<typeof useEventBusStore.getState>>
  }
}

export {}
