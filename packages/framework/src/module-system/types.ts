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

/**
 * The cheap decision context passed to a module's `shouldLoad` predicate. The
 * loader builds it from the current auth/app state; a module returns `true` from
 * `shouldLoad(ctx)` to have its full body downloaded + registered.
 *
 * IMPORTANT: `shouldLoad` is an AUTHORING-TIME predicate. The build plugin
 * (`vite-plugin-module-manifest`) statically LIFTS its source into a manifest
 * baked in the entry chunk, so it can be evaluated WITHOUT downloading the
 * module body (that would defeat the purpose). The predicate therefore may only
 * reference `ctx` and the whitelisted `Permissions` enum — any other free
 * identifier is a build error. Gate on permission via `ctx.can(Permissions.X)`,
 * never a literal permission string.
 */
export interface ModuleLoadContext {
  /** The user has an authenticated session. */
  isAuthenticated: boolean
  /** First-run: no admin account exists yet (the setup flow is active). */
  needsSetup: boolean
  /** The current router path (for route-scoped predicates; usually unused). */
  path: string
  /** The user's flattened active-group permission strings. */
  permissions: string[]
  /** Host platform, so a module can be web-only or desktop-only. */
  platform: 'web' | 'desktop'
  /**
   * True if the user holds ALL of the given permissions (is_admin wildcard
   * short-circuits to true). Pass `Permissions.*` enum members, never literals.
   */
  can: (...perms: string[]) => boolean
}
