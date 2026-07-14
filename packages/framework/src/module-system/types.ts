import type { ReactElement, ComponentType, LazyExoticComponent } from 'react'
import type { UseBoundStore, StoreApi } from 'zustand'

export interface StoreRegistration {
  name: string
  store: UseBoundStore<StoreApi<any>>
}

export interface ModuleMetadata {
  name: string
  version: string
  description?: string
}

// Component Registration (Meta-Framework)
// Components are rendered in App.tsx, sorted by order
export interface ComponentRegistration {
  id: string
  component:
    | ReactElement
    | LazyExoticComponent<ComponentType<any>>
    | (() => Promise<{ default: ComponentType<any> }>)
  order?: number // Rendering order in App.tsx (lower = earlier)
  shouldMount?: () => boolean // React hook to determine if component should be mounted (default: true)
}

// Slot types - extensible slot system with declaration merging
// Modules can declare slots using declaration merging:
// declare module '@ziee/framework/module-system/types' {
//   interface Slots {
//     userGroup: GroupWidget[]
//   }
// }
export interface Slots {}

export type SlotRegistration = Partial<Slots>

export interface AppModule {
  metadata: ModuleMetadata
  registerStores?: () => StoreRegistration[]
  registerComponents?: () => ComponentRegistration[]
  registerDependencies?: () => string[]
  registerSlots?: () => SlotRegistration
  onModuleRegister?: (module: AppModule) => void
  initialize?: () => void | Promise<void>
  cleanup?: () => void | Promise<void>
}
