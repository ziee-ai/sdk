import type {
  AppModule,
  ModuleMetadata,
  StoreRegistration,
  SlotRegistration,
  ComponentRegistration,
} from './module-system/types'

// Base interface - infrastructure modules extend this via declaration merging
export interface CreateModuleOptions {
  metadata: ModuleMetadata
  stores?: StoreRegistration[]
  components?: ComponentRegistration[]
  dependencies?: string[]
  slots?: SlotRegistration
  onModuleRegister?: (module: AppModule) => void
  initialize?: () => void | Promise<void>
  cleanup?: () => void | Promise<void>
}

export function createModule(options: CreateModuleOptions): AppModule {
  return {
    ...options, // Spread all fields (including routes added via declaration merging)
    metadata: options.metadata,
    registerStores: options.stores ? () => options.stores! : undefined,
    registerComponents: options.components
      ? () => options.components!
      : undefined,
    registerDependencies: options.dependencies
      ? () => options.dependencies!
      : undefined,
    registerSlots: options.slots ? () => options.slots! : undefined,
    onModuleRegister: options.onModuleRegister,
    initialize: options.initialize,
    cleanup: options.cleanup,
  }
}
