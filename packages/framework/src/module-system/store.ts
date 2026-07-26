import { create } from 'zustand'
import type { AppModule, Slots, ComponentRegistration } from './types'
import { createStoreProxy } from '../stores'
import { useEventBusStore } from '../events'
import './types-store' // Register ModuleSystem store type

// `keyof Slots` collapses to `never` until an app augments the (empty) base
// `Slots` interface via declaration merging. In this framework package `Slots`
// is empty, so key the slot map by `string`; once an app augments `Slots` the
// conditional resolves to that app's exact union — preserving the stricter
// per-app typing while letting the framework compile standalone.
type SlotKey = [keyof Slots] extends [never] ? string : keyof Slots

interface ModuleSystemState {
  modules: AppModule[]
  stores: Record<string, any>
  slots: Map<SlotKey, any[]>
  components: ComponentRegistration[]
  addComponents: (components: ComponentRegistration[]) => void
  registerModule: (module: AppModule) => void
  /** Register a store proxy directly, by name (idempotent — keeps the first).
   *  Used by whole-store-lazy stores that self-register as a side effect of
   *  their (lazy) chunk loading, instead of being declared in a module's
   *  `stores:` array. Keeps `Stores.<name>` (the compat shim) resolving to the
   *  SAME proxy instance a direct `import { X }` returns. */
  registerStore: (name: string, proxy: any) => void
  initializeModules: () => void
}

/**
 * Names whose proxy is owned by the STORE FILE itself (`registerLazyStore` →
 * `registerStore`), not by this registry. `stores.ts` documents that proxy as
 * the sole owner of init-on-first-access + ref-counted destroy, and it is the
 * instance every consumer holds via a direct `import { X } from './X.store'`.
 * The registry must therefore never destroy or replace it — doing so would give
 * the app two independently ref-counted lifecycles for one store, so `init`
 * (and every `sync:*` listener it registers) could run twice.
 */
const selfOwnedStores = new Set<string>()

/** `import.meta.env` is injected by the bundler and is UNDEFINED under a plain
 *  node runtime (the unit-test resolver), so read it defensively — a dev-only
 *  diagnostic must never be the thing that throws. */
const isDev = (): boolean => {
  try {
    return Boolean((import.meta as { env?: { DEV?: boolean } }).env?.DEV)
  } catch {
    return false
  }
}

export const useModuleSystemStore = create<ModuleSystemState>((set, get) => ({
  modules: [],
  stores: {},
  slots: new Map(),
  components: [],

  addComponents: (components: ComponentRegistration[]) => {
    set(state => ({
      components: [...state.components, ...components],
    }))
  },

  registerStore: (name: string, proxy: any) => {
    selfOwnedStores.add(name)
    set(state => {
      // Idempotent: a store's chunk may be imported by several consumers, but
      // ES-module singletons mean this runs once; guard anyway so a stray
      // double-register never clobbers the live (ref-counted) proxy instance.
      if (state.stores[name]) return state
      return { stores: { ...state.stores, [name]: proxy } }
    })
  },

  registerModule: (module: AppModule) => {
    set(state => {
      // Check if module is already registered
      const existingIndex = state.modules.findIndex(
        m => m.metadata.name === module.metadata.name,
      )

      if (existingIndex !== -1) {
        // In development, allow re-registration for HMR
        if (import.meta.env.DEV) {
          console.log(
            `🔄 Re-registering module for HMR: ${module.metadata.name}`,
          )

          const oldModule = state.modules[existingIndex]
          const newModules = [...state.modules]
          newModules[existingIndex] = module

          // Re-register stores
          const newStores = { ...state.stores }
          if (module.registerStores) {
            const storeRegistrations = module.registerStores()
            storeRegistrations.forEach(reg => {
              // A self-owned proxy (see `selfOwnedStores`) must survive HMR:
              // destroying + replacing it would tear down the LIVE instance every
              // consumer holds and install a second, independently ref-counted
              // one — the same double-`init` / double-`sync:*`-listener hazard
              // the new-module branch below guards against. Stores whose proxy
              // this registry created are still replaced, so HMR keeps working
              // for them.
              if (selfOwnedStores.has(reg.name)) return

              // Destroy old store instance before replacing (HMR cleanup)
              const oldStoreProxy = state.stores[reg.name]
              if (oldStoreProxy?.$?.__destroy__) {
                console.log(`🗑️ Destroying old store for HMR: ${reg.name}`)
                try {
                  oldStoreProxy.$.__destroy__()
                } catch (error) {
                  console.error(
                    `Failed to destroy old store ${reg.name}:`,
                    error,
                  )
                }
              }

              // Create new store proxy
              newStores[reg.name] = createStoreProxy(reg.store)
            })
          }

          // Re-register components - remove old ones first
          const oldComponents = oldModule.registerComponents?.()
          const oldComponentIds = new Set(oldComponents?.map(c => c.id) || [])
          let newComponents = state.components.filter(
            c => !oldComponentIds.has(c.id),
          )

          if (module.registerComponents) {
            const components = module.registerComponents()
            newComponents = [...newComponents, ...components]
            newComponents.sort((a, b) => (a.order ?? 0) - (b.order ?? 0))
          }

          // Re-register slots - rebuild from all modules
          const newSlots = new Map<SlotKey, any[]>()
          for (const mod of newModules) {
            if (mod.registerSlots) {
              const slots = mod.registerSlots()
              for (const [slotName, slotArray] of Object.entries(slots as Record<string, any[]>)) {
                const slot = slotName as SlotKey
                const existing = newSlots.get(slot) || []
                newSlots.set(slot, [...existing, ...slotArray])
              }
            }
          }

          return {
            modules: newModules,
            stores: newStores,
            components: newComponents,
            slots: newSlots,
          }
        } else {
          console.warn(`Module ${module.metadata.name} is already registered`)
          return state
        }
      }

      // Register new module
      const newModules = [...state.modules, module]

      // Register stores
      const newStores = { ...state.stores }
      if (module.registerStores) {
        const storeRegistrations = module.registerStores()
        storeRegistrations.forEach(reg => {
          // REUSE an already-registered proxy. A store authored with
          // `registerLazyStore` self-registers its proxy at import time
          // (`stores.ts`), and that proxy is documented as the SOLE owner of
          // init-on-first-access + ref-counted destroy. Building a second one
          // here for a store also listed in a module's `stores:` array would
          // give it an independent `storeInitialized` flag and ref count — so
          // its `init` (and every `sync:*` listener that registers) could run
          // twice. Idempotent, matching `registerStore` above.
          if (!newStores[reg.name]) {
            newStores[reg.name] = createStoreProxy(reg.store)
          } else if (isDev() && !selfOwnedStores.has(reg.name)) {
            // Two DIFFERENT modules claiming one store name is a real bug, and
            // first-wins would hide it as "my store's actions do nothing".
            // (A self-owned name reaching here is the expected case — the store
            // file registered its own proxy — so it is not warned about.)
            console.warn(
              `[module-system] store name "${reg.name}" is already registered by ` +
                `another module; keeping the first registration. Rename one of them.`,
            )
          }
        })
      }

      // Register components
      const newComponents = [...state.components]
      if (module.registerComponents) {
        const components = module.registerComponents()
        newComponents.push(...components)
        newComponents.sort((a, b) => (a.order ?? 0) - (b.order ?? 0))
      }

      // Register slots — append this module's slot entries to the
      // existing map. Without this, modules registered AFTER
      // `initializeModules()` has already run (e.g., desktop modules
      // loaded by `desktop-loader.ts` post-`loadCoreModules()`) would
      // have their slot entries silently dropped: the new-module
      // branch of this reducer was missing the slot merge that the
      // HMR-rebuild branch above does.
      const newSlots = new Map<SlotKey, any[]>(state.slots)
      if (module.registerSlots) {
        try {
          const slots = module.registerSlots()
          for (const [slotName, slotArray] of Object.entries(slots as Record<string, any[]>)) {
            const slot = slotName as SlotKey
            const existing = newSlots.get(slot) || []
            newSlots.set(slot, [...existing, ...slotArray])
          }
        } catch (error) {
          console.error(
            `Failed to register slots for module ${module.metadata.name}:`,
            error,
          )
        }
      }

      // Call onModuleRegister hook for all existing modules
      state.modules.forEach(existingModule => {
        existingModule.onModuleRegister?.(module)
      })

      // Call new module's hook for all existing modules (catch up)
      if (module.onModuleRegister) {
        state.modules.forEach(existingModule => {
          module.onModuleRegister!(existingModule)
        })
      }

      return {
        modules: newModules,
        stores: newStores,
        components: newComponents,
        slots: newSlots,
      }
    })
  },

  initializeModules: () => {
    const { modules } = get()

    // Step 0: Register core stores in the stores registry
    set(state => ({
      stores: {
        ...state.stores,
        ModuleSystem: createStoreProxy(useModuleSystemStore),
        EventBus: createStoreProxy(useEventBusStore),
      },
    }))

    // Step 1: Run module initialize functions first (creates slot registries)
    for (const module of modules) {
      if (module.initialize) {
        const initialize = module.initialize
        Promise.resolve().then(() => {
          try {
            const result = initialize()
            // If initialize returns a promise, handle it but don't await
            if (result instanceof Promise) {
              result.catch(error =>
                console.error(
                  `Failed to initialize module ${module.metadata.name}:`,
                  error,
                ),
              )
            }
          } catch (error) {
            console.error(
              `Failed to initialize module ${module.metadata.name}:`,
              error,
            )
          }
        })
      }
    }

    // Step 2: Register slots from all modules
    // Rebuild from scratch to prevent duplication during HMR
    set(() => {
      const slotsMap = new Map<SlotKey, any[]>()

      for (const module of modules) {
        if (module.registerSlots) {
          try {
            const slots = module.registerSlots()

            // Register items for each slot
            for (const [slotName, slotArray] of Object.entries(slots as Record<string, any[]>)) {
              const slot = slotName as SlotKey
              const existing = slotsMap.get(slot) || []
              slotsMap.set(slot, [...existing, ...slotArray])
            }
          } catch (error) {
            console.error(
              `Failed to register slots for module ${module.metadata.name}:`,
              error,
            )
          }
        }
      }

      return { slots: slotsMap }
    })
  },
}))
