import type {
  AppModule,
  ModuleLoadContext,
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
  /**
   * Predicate deciding whether this module's body is downloaded + registered.
   * Omit for CORE modules (always loaded at boot). The build plugin lifts this
   * into the entry manifest, so it may only reference `ctx` + the `Permissions`
   * enum (gate on permission with `ctx.can(Permissions.X)`, never a literal).
   * See {@link ModuleLoadContext}.
   */
  shouldLoad?: (ctx: ModuleLoadContext) => boolean
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
