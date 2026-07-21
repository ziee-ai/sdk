import type { StoreApi, UseBoundStore } from 'zustand'
import { useShallow } from 'zustand/react/shallow'
import { useEffect } from 'react'
import { useModuleSystemStore } from './module-system'
import { useEventBusStore } from './events'

// ============================================================================
// Store Proxy - Creates typed store accessors with IntelliSense
// ============================================================================

// Default delay before destroying a store (5 seconds)
const DEFAULT_DESTROY_DELAY_MS = 5000

// Reference tracking interface
export interface ReferenceTracker {
  counts: Map<string | symbol, number>
  totalCount: number
  destroyTimeoutId: NodeJS.Timeout | null
  destroyed: boolean
  addRef: (prop: string | symbol) => void
  removeRef: (prop: string | symbol) => void
  hasRefs: () => boolean
  scheduleDestroy: () => void
  cancelDestroy: () => void
  executeDestroy: () => void
  reset: () => void
}

type RemoveVoid<T> = T extends void ? never : T

type ExtractZustandState<T> = T extends UseBoundStore<infer Store>
  ? Store extends StoreApi<infer State>
    ? RemoveVoid<State> & {
        $: RemoveVoid<State>
        __setState: StoreApi<State>['setState']
      }
    : Store extends { getState(): infer State }
      ? State extends void | infer S
        ? S extends void
          ? never
          : S
        : RemoveVoid<State> & {
            $: RemoveVoid<State>
            __setState: any
          }
      : never
  : never

/**
 * Wraps a Zustand store in a Proxy that gives consumers four distinct
 * access patterns — picked by what `state[prop]` IS at the time of
 * access, not by where the access happens at runtime:
 *
 *   1. **Special properties** (`$`, `__setState`, `__refCount`,
 *      `__refTracker`, `__destroyed`) — return synchronously, no hooks.
 *      Safe to read anywhere. `$` is the single handler-side snapshot
 *      escape: `Stores.X.$.field` reads `getState()` with no hooks.
 *   2. **Function values (actions)** — returned resolved from
 *      `getState()`, no hooks. Safe to call ANYWHERE — render, event
 *      handlers, async callbacks, module-init — with NO `$` and NO
 *      snapshot ceremony: `Stores.X.doThing()` just works.
 *   3. **Nested store proxies** (objects with `__refTracker`) — return
 *      directly, no hooks. Safe to read anywhere. (The nested proxy
 *      handles its own reactivity.)
 *   4. **Plain state values** — call `useEffect` + `useStore(useShallow(...))`
 *      under the hood. The return value is reactive: the component
 *      re-renders when the value changes.
 *
 * The implicit contract: **path 4 must only be entered from inside a
 * React component render** (same rule that applies to any custom hook).
 * Whether path 4 is taken is determined by the property's TYPE, which
 * is stable across renders for any given `(store, prop)` pair, so hook
 * order is preserved per-component the same way it is for `useState` /
 * `useEffect`.
 *
 * In practice:
 *   - `Stores.Auth.login()` — action, safe anywhere (render + handlers).
 *   - `const { user } = Stores.Auth` — reactive state read, MUST be
 *     inside a component / `use*` hook. Re-renders on change.
 *   - `Stores.Auth.$.user` — hook-free snapshot read, safe in handlers.
 *
 * Reading a non-special, non-action state value from non-component
 * code throws React's `Invalid hook call` at runtime — use `$` there.
 * The `.__state` alias for `$` was removed; a Biome guardrail
 * (`biome-plugins/no-store-internal-state.grit`) bans its reintroduction.
 *
 * (audit 01 B-1)
 */
export const createStoreProxy = <T extends UseBoundStore<StoreApi<any>>>(
  useStore: T,
): Readonly<ExtractZustandState<T>> => {
  const propInitCheck = new Map<string | symbol, boolean>()
  let storeInitialized = false

  // Reference tracking with delayed destruction
  const refTracker: ReferenceTracker = {
    counts: new Map<string | symbol, number>(),
    totalCount: 0,
    destroyTimeoutId: null,
    destroyed: false,

    addRef: (prop: string | symbol) => {
      // If destruction is pending, cancel it (user is accessing again!)
      if (refTracker.destroyTimeoutId !== null) {
        if (import.meta.env.DEV) {
          console.log('🔄 Cancelling destruction - store accessed again')
        }
        refTracker.cancelDestroy()
      }

      // If store was destroyed, reset for re-initialization
      if (refTracker.destroyed) {
        if (import.meta.env.DEV) {
          console.log('🔄 Re-initializing previously destroyed store')
        }
        refTracker.reset()
      }

      const current = refTracker.counts.get(prop) || 0
      refTracker.counts.set(prop, current + 1)
      refTracker.totalCount++
    },

    removeRef: (prop: string | symbol) => {
      const current = refTracker.counts.get(prop) || 0
      if (current > 0) {
        refTracker.counts.set(prop, current - 1)
        refTracker.totalCount--

        // Schedule destruction when no active references
        if (refTracker.totalCount === 0) {
          refTracker.scheduleDestroy()
        }
      }
    },

    hasRefs: () => refTracker.totalCount > 0,

    scheduleDestroy: () => {
      const state = useStore.getState()

      // Only schedule if store has __destroy__ method
      if (!state.__destroy__ || typeof state.__destroy__ !== 'function') {
        return
      }

      // Get custom delay from store or use default
      const delay = (state as any).__destroyDelay__ || DEFAULT_DESTROY_DELAY_MS

      if (import.meta.env.DEV) {
        console.log(
          `⏳ Scheduling store destruction in ${delay}ms (no active references)`,
        )
      }

      refTracker.destroyTimeoutId = setTimeout(() => {
        refTracker.executeDestroy()
      }, delay)
    },

    cancelDestroy: () => {
      if (refTracker.destroyTimeoutId !== null) {
        clearTimeout(refTracker.destroyTimeoutId)
        refTracker.destroyTimeoutId = null
      }
    },

    executeDestroy: () => {
      const state = useStore.getState()

      if (import.meta.env.DEV) {
        console.log('🗑️ Executing store destruction (delay expired)')
      }

      try {
        // Call store's custom destroy hook
        const result = (state as any).__destroy__()
        if (result instanceof Promise) {
          result.catch((err: any) => {
            console.error('Store __destroy__ error:', err)
          })
        }

        // Mark as destroyed and clear initialization state immediately
        refTracker.destroyed = true
        refTracker.destroyTimeoutId = null

        // Clear initialization state so store can be re-initialized if accessed again
        propInitCheck.clear()
        storeInitialized = false

        if (import.meta.env.DEV) {
          console.log('✅ Store destroyed successfully')
        }
      } catch (err) {
        console.error('Store __destroy__ error:', err)
      }
    },

    reset: () => {
      // Reset initialization flags for re-initialization
      propInitCheck.clear()
      storeInitialized = false
      refTracker.destroyed = false
      refTracker.totalCount = 0
      refTracker.counts.clear()

      if (import.meta.env.DEV) {
        console.log('🔄 Store tracker reset for re-initialization')
      }
    },
  }

  return new Proxy({} as Readonly<ExtractZustandState<T>>, {
    get: (_, prop) => {
      // Special properties.
      // `$` is the SOLE handler-side snapshot escape: `Stores.X.$.field`
      // reads getState() with NO hooks (safe in event handlers / async).
      // (The old `__state` alias was removed — actions no longer need any
      // snapshot escape; a Biome guardrail bans `.__state` reintroduction.)
      if (prop === '$') {
        return useStore.getState()
      }
      if (prop === '__setState') {
        return useStore.setState.bind(useStore)
      }
      if (prop === '__refCount') {
        return refTracker.totalCount
      }
      if (prop === '__refTracker') {
        return refTracker
      }
      if (prop === '__destroyed') {
        return refTracker.destroyed
      }

      const state = useStore.getState()

      // Store-level initialization (only if not destroyed)
      if (!storeInitialized && state.__init__?.__store__) {
        if (typeof state.__init__.__store__ === 'function') {
          state.__init__.__store__()
        }
        storeInitialized = true
      }

      // Property-specific initialization
      const isInit = propInitCheck.get(prop) || false
      if (!isInit) {
        if (state.__init__ && typeof state.__init__[prop] === 'function') {
          state.__init__[prop]()
        }
        propInitCheck.set(prop, true)
      }

      // If the property is a function (action), return it resolved from
      // getState(), hook-free. This is what makes actions callable in ANY
      // context — render AND event handlers / async — with no `$`/snapshot
      // ceremony. The reference is stable across renders (actions are built
      // once), so no Rules-of-Hooks concern.
      const value = (state as any)[prop]
      if (typeof value === 'function') {
        return value
      }

      // Check if value is a nested store proxy (has __refTracker)
      // Return directly without hooks - nested stores manage their own reactivity
      // This allows accessing nested stores from event handlers without hook errors
      // Note: Use property access instead of 'in' operator because Proxy's 'in' trap
      // checks the target object, not the handler
      if (value && typeof value === 'object' && (value as any).__refTracker) {
        return value
      }

      // For state values, track reference with useEffect
      // eslint-disable-next-line react-hooks/rules-of-hooks
      useEffect(() => {
        // Component mounted and accessing this property
        refTracker.addRef(prop)

        // Cleanup when component unmounts
        return () => {
          refTracker.removeRef(prop)
        }
      }, []) // Empty deps - only run on mount/unmount

      // Return reactive value via hook
      return useStore(
        useShallow((state: ExtractZustandState<T>) => (state as any)[prop]),
      )
    },
  })
}

// ============================================================================
// Registered Stores - Dynamic store registry with IntelliSense
// ============================================================================

// Helper type to wrap store state with proxy methods
export type StoreProxy<T> = Readonly<
  T & {
    /** Handler-side snapshot: `Stores.X.$.field` (no hooks, safe anywhere).
     *  The only escape needed for READING state in a handler — actions are
     *  always callable directly (`Stores.X.action()`), no `$` required. */
    $: T
    __setState: (partial: Partial<T> | ((state: T) => Partial<T>)) => void
    __refCount: number
    __refTracker: ReferenceTracker
    __destroyed: boolean
  }
>

// This interface will be augmented by modules via declaration merging
export interface RegisteredStores {
  // Modules will add their store types here via:
  // declare module '@ziee/framework/stores' {
  //   interface RegisteredStores {
  //     Auth: StoreProxy<{ user: User, isAuthenticated: boolean, ... }>
  //   }
  // }
}

// NOTE: the global `Stores` proxy has been REMOVED. Every store is now consumed
// via its direct handle (`import { X } from '.../X.store'`, the proxy returned
// by `registerLazyStore` / `defineStore`) — so a page only pulls the stores it
// actually uses (O(page-stores) boot instead of O(all-stores)). The module-system
// registry below still tracks stores for lifecycle (init/destroy on module
// load/unload + ref-counting); it simply no longer backs a global `.X` facade.

/**
 * WHOLE-STORE-LAZY registration. A store file calls this at module scope:
 *
 *   export const Users = registerLazyStore(defineStore('Users', { … }))
 *
 * It builds the lifecycle proxy ONCE (via `createStoreProxy`) and self-registers
 * it, then RETURNS that proxy as the importable handle. So:
 *   - `import { Users } from './Users.store'` → the reactive/lifecycle proxy
 *     (the store's code rides THIS chunk, loaded only where imported → lazy).
 *   - `Stores.Users` (compat shim) → the SAME proxy instance (single ref-count).
 *
 * The proxy is the sole owner of init-on-first-access + ref-counted destroy, so
 * whether you reach it via the import or the shim, the lifecycle is identical.
 * `defineStore`'s existing `{ name, store }` return is unchanged — this wraps it,
 * so the 89 un-migrated modules that use `stores: [...]` are untouched.
 */
export function registerLazyStore<
  H extends { name: string; store: UseBoundStore<StoreApi<any>> },
>(handle: H): StoreProxy<ExtractZustandState<H['store']>> {
  const proxy = createStoreProxy(handle.store)
  useModuleSystemStore.getState().registerStore(handle.name, proxy as any)
  return proxy as StoreProxy<ExtractZustandState<H['store']>>
}

// Type helper for accessing store state
export type StoresType = RegisteredStores

/**
 * Direct handles for the framework-infra stores (were `Stores.EventBus` /
 * `Stores.ModuleSystem`). Import these instead of going through a global.
 *
 * LAZY on purpose: `stores.ts` and `./module-system` (which provides
 * `useModuleSystemStore` + imports `createStoreProxy` from here) form an import
 * cycle. Whichever module the bundler evaluates first, the other's exports can
 * still be in the temporal dead zone when this file's top-level runs — so an
 * eager `createStoreProxy(useModuleSystemStore)` would capture `undefined` and
 * every `ModuleSystem.<field>` read would throw `getState of undefined`. Building
 * the inner proxy on FIRST ACCESS (by which point both modules are fully
 * evaluated) resolves the cycle — this is the same laziness the removed global
 * `Stores.X` proxy relied on.
 */
function lazyStoreProxy<T extends UseBoundStore<StoreApi<any>>>(
  getStore: () => T,
): Readonly<ExtractZustandState<T>> {
  let inner: Readonly<ExtractZustandState<T>> | null = null
  return new Proxy({} as Readonly<ExtractZustandState<T>>, {
    get: (_t, prop) => {
      if (inner == null) inner = createStoreProxy(getStore())
      return (inner as Record<string | symbol, unknown>)[prop]
    },
  })
}

export const EventBus = lazyStoreProxy(() => useEventBusStore)
export const ModuleSystem = lazyStoreProxy(() => useModuleSystemStore)
