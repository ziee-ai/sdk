import type { StoreApi, UseBoundStore } from 'zustand'
import { useShallow } from 'zustand/react/shallow'
import { useEffect } from 'react'
import { useModuleSystemStore } from './module-system'
import { useEventBusStore } from './events'

// ============================================================================
// Store Proxy - Creates typed store accessors with IntelliSense
// ============================================================================

// `import.meta.env` is injected by the bundler and is UNDEFINED under a plain
// node runtime (the SDK unit-test resolver) — read it defensively so the
// destroy lifecycle (`scheduleDestroy` / `executeDestroy`) is exercisable
// under `node --test`; a dev-only diagnostic must never be the thing that
// throws. Same guard as `module-system/store.ts::isDev()`.
const IS_DEV = ((): boolean => {
  try {
    return Boolean((import.meta as { env?: { DEV?: boolean } }).env?.DEV)
  } catch {
    return false
  }
})()

// Default delay before destroying a store (5 seconds)
const DEFAULT_DESTROY_DELAY_MS = 5000

// ============================================================================
// Pins — an OWNER-held reference that is not a render read.
//
// The render ref-count answers "is a component showing this store right now?".
// An app can know something the render tree cannot: that a page the user just
// LEFT is still one Back (or Forward) away. A pin is that knowledge, stated as
// a reference: `pinStore(proxy, owner)` keeps the store alive while ANY owner
// holds it, exactly the way a render ref does — a store is destroyed only when
// it has no render refs AND no pins. Unpinning the last owner while nothing
// renders the store resumes the ordinary delayed destroy.
//
// A pinned store that nothing renders is QUIET: the proxy tells the store
// (`__visibility__`, below) and the store-kit defers its event handlers until
// it is shown again (see `store-kit.ts`, "Quiet while hidden"). The SDK owns
// the mechanism only; WHICH stores an app pins, and for how long, is app policy.
// ============================================================================

/** The visibility facts the proxy hands a store whenever either changes:
 *  `visible` = at least one render ref; `pinned` = at least one owner pin. */
export interface StoreVisibility {
  visible: boolean
  pinned: boolean
}

/** Observes every store's render activity (0 → ≥1 render refs = active,
 *  ≥1 → 0 = inactive). ONE app-installed observer — the hook an app uses to
 *  learn which stores the page it is showing actually rendered. */
export type StoreActivityObserver = (proxy: object, active: boolean, name: string | null) => void

let activityObserver: StoreActivityObserver | null = null

/** Install (or clear, with `null`) the store-activity observer. */
export function setStoreActivityObserver(observer: StoreActivityObserver | null): void {
  activityObserver = observer
}

/** Every proxy that currently holds ≥1 render ref (an observer installed after
 *  boot needs the stores that were ALREADY on screen). */
const liveProxies = new Set<object>()

/** The proxies currently rendered somewhere (a snapshot copy). */
export function liveStoreProxies(): object[] {
  return [...liveProxies]
}

function trackerOf(proxy: object): ReferenceTracker | null {
  const t = (proxy as { __refTracker?: ReferenceTracker }).__refTracker
  return t && typeof t.pin === 'function' ? t : null
}

/** Pin `proxy` on behalf of `owner` (idempotent per owner). A pinned store is
 *  never destroyed; a pending destroy is cancelled. Returns false when the
 *  value is not a store proxy. */
export function pinStore(proxy: object, owner: string): boolean {
  const t = trackerOf(proxy)
  if (!t) return false
  t.pin(owner)
  return true
}

/** Release `owner`'s pin. When it was the last pin and nothing renders the
 *  store, the ordinary delayed destroy is scheduled. */
export function unpinStore(proxy: object, owner: string): void {
  trackerOf(proxy)?.unpin(owner)
}

// Reference tracking interface
export interface ReferenceTracker {
  counts: Map<string | symbol, number>
  totalCount: number
  destroyTimeoutId: NodeJS.Timeout | null
  destroyed: boolean
  /** The owners currently pinning this store (see `pinStore`). */
  pins: Set<string>
  /** The registered store name, when `registerLazyStore` built the proxy. */
  name: string | null
  pin: (owner: string) => void
  unpin: (owner: string) => void
  isPinned: () => boolean
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
  // The proxy object itself (assigned below) — the identity the activity
  // observer and the pin API key on.
  let self: object = {}
  // The last visibility pair handed to the store (`__visibility__`), so a
  // store hears each CHANGE once.
  let lastVisible = false
  let lastPinned = false

  // Tell the store when it becomes shown / hidden / pinned / unpinned. The
  // store-kit uses this to keep a pinned-but-hidden store QUIET. A store
  // without the hook (a raw zustand store) is unaffected.
  const notifyVisibility = (): void => {
    const visible = refTracker.totalCount > 0
    const pinned = refTracker.pins.size > 0
    if (visible === lastVisible && pinned === lastPinned) return
    lastVisible = visible
    lastPinned = pinned
    const state = useStore.getState() as { __visibility__?: (v: StoreVisibility) => void } | undefined
    if (!state || typeof state.__visibility__ !== 'function') return
    try {
      state.__visibility__({ visible, pinned })
    } catch (err) {
      console.error('Store __visibility__ error:', err)
    }
  }

  // Reference tracking with delayed destruction
  const refTracker: ReferenceTracker = {
    counts: new Map<string | symbol, number>(),
    totalCount: 0,
    destroyTimeoutId: null,
    destroyed: false,

    addRef: (prop: string | symbol) => {
      // If destruction is pending, cancel it (user is accessing again!)
      if (refTracker.destroyTimeoutId !== null) {
        if (IS_DEV) {
          console.log('🔄 Cancelling destruction - store accessed again')
        }
        refTracker.cancelDestroy()
      }

      // If store was destroyed, reset for re-initialization
      if (refTracker.destroyed) {
        if (IS_DEV) {
          console.log('🔄 Re-initializing previously destroyed store')
        }
        refTracker.reset()
      }

      const current = refTracker.counts.get(prop) || 0
      refTracker.counts.set(prop, current + 1)
      refTracker.totalCount++
      if (refTracker.totalCount === 1) {
        liveProxies.add(self)
        activityObserver?.(self, true, refTracker.name)
      }
      notifyVisibility()
    },

    removeRef: (prop: string | symbol) => {
      const current = refTracker.counts.get(prop) || 0
      if (current > 0) {
        refTracker.counts.set(prop, current - 1)
        refTracker.totalCount--

        if (refTracker.totalCount === 0) {
          liveProxies.delete(self)
          activityObserver?.(self, false, refTracker.name)
          // Schedule destruction when no active references — and no owner
          // still pins the store (a pinned store is kept, quiet).
          if (refTracker.pins.size === 0) {
            refTracker.scheduleDestroy()
          }
        }
        notifyVisibility()
      }
    },

    hasRefs: () => refTracker.totalCount > 0,

    pins: new Set<string>(),
    name: null,

    pin: (owner: string) => {
      if (refTracker.pins.has(owner)) return
      refTracker.pins.add(owner)
      // A pin is a reference: a pending destroy is cancelled exactly as a
      // render ref cancels it.
      refTracker.cancelDestroy()
      notifyVisibility()
    },

    unpin: (owner: string) => {
      if (!refTracker.pins.delete(owner)) return
      if (
        refTracker.pins.size === 0 &&
        refTracker.totalCount === 0 &&
        !refTracker.destroyed &&
        refTracker.destroyTimeoutId === null
      ) {
        refTracker.scheduleDestroy()
      }
      notifyVisibility()
    },

    isPinned: () => refTracker.pins.size > 0,

    scheduleDestroy: () => {
      const state = useStore.getState()

      // Only schedule if the store still exists and has a __destroy__ method.
      // `state` can be undefined for a per-instance (local) store whose Zustand
      // store was already disposed before this ref-count-triggered teardown ran
      // (rapid mount/unmount, e.g. split-chat panes). An unguarded
      // `state.__destroy__` there throws → escapes to the router error boundary →
      // blank page.
      if (!state || !state.__destroy__ || typeof state.__destroy__ !== 'function') {
        return
      }

      // Get custom delay from store or use default
      const delay = (state as any).__destroyDelay__ || DEFAULT_DESTROY_DELAY_MS

      if (IS_DEV) {
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
      // `executeDestroy` is called directly (module teardown, tests) while a
      // destroy timer armed by `scheduleDestroy` may still be pending.
      // `cancelDestroy` was the only thing that cleared the timer; this call
      // used to null the handle WITHOUT clearing it, so the stale timeout
      // later fired and ran the destroy hook AGAIN on a store a live page was
      // using. Clear any pending timer first, always.
      if (refTracker.destroyTimeoutId !== null) {
        clearTimeout(refTracker.destroyTimeoutId)
        refTracker.destroyTimeoutId = null
      }

      // Clear initialization state so the store re-initialises on the next
      // access. This was unconditional on every direct destroy, and a page
      // that comes back after teardown must re-run `init` on its FIRST render
      // (before any `addRef`/`reset`), so it happens even when the destroy
      // hook itself is not re-run below.
      propInitCheck.clear()
      storeInitialized = false

      // Idempotent against a destroy already run (the pending timer fired
      // first, or there was no `__destroy__` to run): a second teardown must
      // not run the destroy hook again on a store that is already gone.
      if (refTracker.destroyed) {
        return
      }

      const state = useStore.getState()

      // The store may have been disposed between scheduling and this timeout
      // firing (a local store's Zustand instance torn down on unmount), leaving
      // `getState()` undefined — nothing left to destroy.
      if (!state || typeof (state as any).__destroy__ !== 'function') {
        refTracker.destroyed = true
        return
      }

      if (IS_DEV) {
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

        // Mark as destroyed
        refTracker.destroyed = true

        if (IS_DEV) {
          console.log('✅ Store destroyed successfully')
        }
      } catch (err) {
        console.error('Store __destroy__ error:', err)
      }
    },

    reset: () => {
      // Reset initialization flags for re-initialization
      // Pins are OWNER references, not render reads: they survive a reset.
      propInitCheck.clear()
      storeInitialized = false
      refTracker.destroyed = false
      if (refTracker.totalCount > 0) {
        liveProxies.delete(self)
        activityObserver?.(self, false, refTracker.name)
      }
      refTracker.totalCount = 0
      refTracker.counts.clear()
      notifyVisibility()

      if (IS_DEV) {
        console.log('🔄 Store tracker reset for re-initialization')
      }
    },
  }

  const proxy = new Proxy({} as Readonly<ExtractZustandState<T>>, {
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
  self = proxy
  return proxy
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
  ;(proxy as unknown as { __refTracker: ReferenceTracker }).__refTracker.name = handle.name
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
