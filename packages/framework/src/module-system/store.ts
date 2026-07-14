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
  initializeModules: () => void
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
          newStores[reg.name] = createStoreProxy(reg.store)
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
