import { create, useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import {
  persist as persistMiddleware,
  type PersistOptions,
  subscribeWithSelector,
} from 'zustand/middleware'
import { immer as immerMiddleware } from 'zustand/middleware/immer'
import { useShallow } from 'zustand/react/shallow'
import { useEffect, useRef } from 'react'
import type { Mutate, StoreApi, UseBoundStore } from 'zustand'
import { useEventBusStore } from './events'
import type { AppEvents, EventHandler, Unsubscribe } from './events/types'
import { createStoreProxy, type StoreProxy } from './stores'

// ============================================================================
// store-kit — thin authoring layer over the existing Zustand + Stores.X proxy.
//
// Goal: kill the per-store ceremony (create()(subscribeWithSelector(immer(…))),
// the __init__/__destroy__ magic keys, the GROUP string + manual
// removeGroupListeners, the {name,store} runtime entry) WITHOUT changing how
// consumers read stores. `const { a, b } = Stores.X` and `Stores.X.action()`
// are unchanged; `Stores.X.$` is the clean handler-side snapshot (see the proxy).
// ============================================================================

/** Anything with the subscribeWithSelector 3-arg `subscribe` + `getState`
 *  (a raw Zustand store, or another store's `.store`). Used by `watch`. */
export interface Subscribable<S> {
  getState: () => S
  subscribe: {
    (listener: (state: S, prev: S) => void): Unsubscribe
    <U>(
      selector: (state: S) => U,
      listener: (u: U, prev: U) => void,
      options?: { equalityFn?: (a: U, b: U) => boolean; fireImmediately?: boolean },
    ): Unsubscribe
  }
}

/** The `set` handed to actions/init. Accepts an immer-draft mutator (when
 *  `immer: true`) OR a partial / merge-updater (plain Zustand). Typed loosely
 *  on purpose so both styles compile; the runtime middleware enforces the real
 *  contract per store. */
export type StoreSet<S> = (
  partial: Partial<S> | ((draft: S) => Partial<S> | void),
) => void

/** Base toolkit passed to `init` (plus `actions`, added in StoreConfig). */
export interface StoreInitCtx<State> {
  set: StoreSet<State>
  get: () => State
  /** Subscribe to an app event (auto-grouped by the store name, auto-cleaned). */
  on: <K extends keyof AppEvents>(
    event: K,
    handler: EventHandler<AppEvents[K]>,
  ) => void
  /** React to another store's slice (replaces raw `useX.subscribe`, auto-cleaned).
   *  Fires immediately by default. */
  watch: <S, U>(
    store: Subscribable<S>,
    selector: (s: S) => U,
    cb: (value: U, prev: U) => void,
    opts?: { fireImmediately?: boolean; equalityFn?: (a: U, b: U) => boolean },
  ) => void
  /** Register an arbitrary teardown to run on store destroy — the escape hatch
   *  for imperative resources (an SSE AbortController, a timer) that aren't an
   *  `on`/`watch` subscription. Runs alongside the auto-cleaned listeners. */
  onCleanup: (fn: () => void) => void
}

// ============================================================================
// Lazy actions — per-action code splitting.
//
// Most of a store's WEIGHT is its actions (API calls + logic). `lazyActions`
// lets each action live in its OWN file/chunk, downloaded on first call (or
// `preload()`) instead of riding the store's eager chunk. The store's state is
// eager + always present (so `Stores.X.field` never `undefined`), only the
// action CODE is deferred. All lazy actions are async (they may need to `await`
// their own chunk); trivial synchronous setters stay in `actions`.
//
//   // Users.ts
//   defineStore('Users', {
//     state: { list: [] },
//     lazyActions: {
//       loadUsers:  () => import('./actions/loadUsers'),
//       deleteUser: () => import('./actions/deleteUser'),
//     },
//   })
//   // actions/loadUsers.ts  →  export default (set, get) => async () => { … }
//
//   Users.loadUsers()            // loads only loadUsers' chunk, then runs
//   Users.deleteUser.preload()   // warm the chunk on hover/intent
// ============================================================================

/** A lazily-loaded async action module: its default export is a factory
 *  `(set, get) => (...args) => Promise<Ret>`. */
export type LazyActionFactory<State, Args extends any[], Ret> = (
  set: StoreSet<State>,
  get: () => State,
) => (...args: Args) => Promise<Ret>

/** A thunk that dynamic-imports one action module (→ its own chunk). */
export type LazyActionLoader<State> = () => Promise<{
  default: LazyActionFactory<State, any[], any>
}>

export type LazyActionsConfig<State> = Record<string, LazyActionLoader<State>>

/** The callable a lazy action becomes on the store: same signature as the
 *  underlying action, plus `.preload()` to warm its chunk without invoking it. */
export type LazyDispatcher<L> = L extends () => Promise<{
  default: (set: any, get: any) => (...args: infer A) => infer R
}>
  ? ((...args: A) => R extends Promise<any> ? R : Promise<Awaited<R>>) & {
      preload: () => Promise<void>
    }
  : never

export type LazyDispatchers<M> = { [K in keyof M]: LazyDispatcher<M[K]> }

// ============================================================================
// Folder-convention auto-registration.
//
// `import.meta.glob('./actions/*.ts')` yields `{ './actions/foo.ts': loader }`.
// A store passes THAT as `config.actions` — store-kit derives the action name
// from each file's basename (no hand-written map). `_`-prefixed files are
// INTERNAL helpers (shared factories imported by actions) and are NOT registered
// as dispatched actions. Types come from a generated `actions.gen.ts` type map
// (see `gen:store-actions`), supplied as the `AM` generic.
// ============================================================================

/** The record `import.meta.glob('./actions/*.ts')` produces (path → lazy loader).
 *  Vite types the loaders as `() => Promise<unknown>`, so the value side is loose;
 *  the concrete action types come from the `AM` generic (generated actions.gen.ts). */
export type ActionGlob = Record<string, () => Promise<any>>

/** Dispatcher type derived from ONE action factory (as `typeof import('./x')['default']`). */
export type FactoryDispatcher<F> = F extends (
  set: any,
  get: any,
) => (...args: infer A) => infer R
  ? ((...args: A) => R extends Promise<any> ? R : Promise<Awaited<R>>) & {
      preload: () => Promise<void>
    }
  : never

/** The generated `actions.gen.ts` type map (name → factory type) → dispatchers. */
export type DispatchersFromTypeMap<M> = { [K in keyof M]: FactoryDispatcher<M[K]> }

/** EAGER glob form: `import.meta.glob('./actions/*.ts', { eager: true })` — its
 *  values are already-loaded module namespaces (`{ default: factory }`), not the
 *  lazy loader thunks (`() => import()`). Used by stores with SYNCHRONOUS
 *  selectors (a `getFoo(): string` read in render), which can't be deferred
 *  behind a dynamic import. Same folder structure, sync actions.
 *
 *  Typed as `Record<string, unknown>` because that is exactly what Vite infers
 *  for an un-parameterized eager glob (the lazy glob is `Record<string, () =>
 *  Promise<...>>`), so this is the type that DISTINGUISHES the eager overload
 *  from the lazy one at the call site. The action SIGNATURES come from the `AM`
 *  generic (`actions.gen.ts`), not from these values, so no precision is lost. */
export type EagerActionGlob = Record<string, unknown>

/** Eager (synchronous) factory → its resolved action, signature preserved
 *  VERBATIM (NO Promise-wrapping, NO `.preload`) — the opposite of
 *  `FactoryDispatcher`, so a `getFoo(): string` stays `() => string`. */
export type EagerFactoryDispatcher<F> = F extends (
  set: any,
  get: any,
) => infer A
  ? A
  : never
export type EagerDispatchersFromTypeMap<M> = {
  [K in keyof M]: EagerFactoryDispatcher<M[K]>
}

/** True when `actions` is a glob record (object) rather than the eager factory (function). */
function isActionGlob(a: unknown): a is ActionGlob {
  return !!a && typeof a === 'object'
}

/** An eager glob's values are loaded module objects (`{default}`), not the lazy
 *  loader thunks. Empty glob → treated as lazy (a no-op either way). */
function isEagerGlob(glob: ActionGlob): boolean {
  for (const path of Object.keys(glob)) return typeof glob[path] !== 'function'
  return false
}

/** Eager glob → a plain `actions` factory (basename→factory(set,get)), built
 *  SYNCHRONOUSLY. Drops `_`-prefixed internal-helper files (as the lazy form does). */
function globToEagerActions(
  glob: ActionGlob,
): (set: any, get: any) => Record<string, any> {
  const factories: Record<string, (set: any, get: any) => any> = {}
  for (const path of Object.keys(glob)) {
    const base = path.slice(path.lastIndexOf('/') + 1).replace(/\.[tj]sx?$/, '')
    if (!base.startsWith('_')) factories[base] = (glob[path] as any).default
  }
  return (set: any, get: any) => {
    const out: Record<string, any> = {}
    for (const name of Object.keys(factories)) out[name] = factories[name](set, get)
    return out
  }
}

/** Glob record → basename→loader map (the internal lazyActions form). Drops
 *  `_`-prefixed internal-helper files. */
function globToLazyActions(glob: ActionGlob): Record<string, LazyActionLoader<any>> {
  const out: Record<string, LazyActionLoader<any>> = {}
  for (const path of Object.keys(glob)) {
    const base = path.slice(path.lastIndexOf('/') + 1).replace(/\.[tj]sx?$/, '')
    if (!base.startsWith('_')) out[base] = glob[path] as LazyActionLoader<any>
  }
  return out
}

/** If `config.actions` is a glob, normalize it: an EAGER glob → a synchronous
 *  `actions` factory (sync selectors preserved); a LAZY glob → `lazyActions`
 *  (basename keys, deferred per-action chunks). */
function normalizeGlobConfig<C extends { actions?: unknown; lazyActions?: unknown }>(
  config: C,
): C {
  if (typeof config.actions === 'function' || !isActionGlob(config.actions)) return config
  if (isEagerGlob(config.actions))
    return { ...config, actions: globToEagerActions(config.actions) }
  return { ...config, actions: undefined, lazyActions: globToLazyActions(config.actions) }
}

/** Browser-idle scheduler: run `cb` in a spare frame gap, off the render/critical
 *  path. `{ timeout }` guarantees it still fires under a busy main thread (e.g. a
 *  long SSE token stream keeps eating idle time). No-op off-browser (SSR/tests). */
export function warmIdle(cb: () => void): void {
  if (typeof window === 'undefined') return
  if (typeof requestIdleCallback !== 'undefined') requestIdleCallback(cb, { timeout: 2000 })
  else setTimeout(cb, 200)
}

/** BAKED-IN PREFETCH: on init, warm every lazy-action chunk on idle so the store's
 *  interactions are instant — with NO per-store `.preload()` wiring. Actions already
 *  invoked in `init` (hot) are a cached no-op; the rest fetch in the background. */
function autoWarmLazyActions(lazyDispatchers: Record<string, any>): void {
  const keys = Object.keys(lazyDispatchers)
  if (!keys.length) return
  warmIdle(() => {
    for (const k of keys) {
      try {
        lazyDispatchers[k].preload?.()
      } catch {
        /* best-effort */
      }
    }
  })
}

export interface StoreConfig<
  State extends object,
  Actions extends object,
  LA extends LazyActionsConfig<State> = {},
> {
  /** Per-action lazy loaders — each value dynamic-imports one action module.
   *  The dispatchers are merged into the store's actions (typed from the import),
   *  so `Stores.X.<name>(...)` calls stay fully typed while the code is deferred. */
  lazyActions?: LA
  /** Draft-mutation setters (`set(d => { d.x = 1 })`). Default false → plain
   *  Zustand shallow-merge (`set(s => ({ x: 1 }))`), so merge-style stores like
   *  Chat migrate with NO change to their setters. */
  immer?: boolean
  /** Opt-in localStorage persistence — the plain zustand `persist` options
   *  (`name`, `storage`, `partialize`, `version`, `migrate`, `merge`,
   *  `onRehydrateStorage`, `skipHydration`). Applied UNDER subscribeWithSelector
   *  (so the 3-arg subscribe overload survives) and OVER immer (when enabled).
   *  `partialize` sees only `State` (the data fields — never actions/lifecycle,
   *  which are always restored from the builder); typing it over `State` rather
   *  than the full state is also what keeps `Actions` cleanly inferrable from
   *  the `actions` return instead of collapsing to `object`. */
  persist?: PersistOptions<State, any>
  state: State
  // `get`/`set` are typed over STATE only (not State & Actions) — that keeps
  // `Actions` out of any parameter position, so it's inferred cleanly from the
  // return (and consumers get full action typing). An action that calls another
  // action does so via a local closure; `init` gets the resolved `actions`.
  actions?: (set: StoreSet<State>, get: () => State) => Actions
  /** Runs once on first access (global) / on mount (local). Listener + cross-store
   *  wiring goes here via `on` / `watch`; all of it auto-unsubscribes on destroy.
   *  Gets the resolved `actions` so it can call them (typed). */
  init?: (
    ctx: StoreInitCtx<State> & { actions: Actions & LazyDispatchers<LA> },
  ) => void
}

/** Internal lifecycle keys the Stores proxy already understands. */
interface Lifecycle {
  __init__: { __store__: () => void }
  // May be async: the callers fire-and-forget it (React cleanup ignores the
  // return; the global ref-count teardown `.catch()`es a returned Promise), so a
  // store whose teardown awaits a lazy action (e.g. Chat saving pane state before
  // destroy) is supported.
  __destroy__: () => void | Promise<void>
}

export type FullStoreState<State, Actions> = State & Actions & Lifecycle

/** A registered global store: shaped exactly as `StoreRegistration`
 *  ({ name, store }) so `createModule({ stores: [handle] })` accepts it. */
/** The store bound-hook type, carrying the `subscribeWithSelector` mutator so
 *  the 3-arg `store.subscribe(selector, cb, opts)` overload is preserved for
 *  consumers (chat extensions, `watch`) — every store-kit store is wrapped in
 *  subscribeWithSelector at runtime. */
export type BoundStore<FullState> = UseBoundStore<
  Mutate<StoreApi<FullState>, [['zustand/subscribeWithSelector', never]]>
>

/** A registered global store: shaped exactly as `StoreRegistration`
 *  ({ name, store }) so `createModule({ stores: [handle] })` accepts it. */
export interface StoreHandle<FullState> {
  name: string
  store: BoundStore<FullState>
}

/** Build the state object + wire the auto-cleanup lifecycle. Shared by the
 *  global (`defineStore`) and local (`defineLocalStore`) factories. */
function makeBuilder<State extends object, Actions extends object>(
  groupName: string,
  config: StoreConfig<State, Actions>,
) {
  return (set: any, get: any): FullStoreState<State, Actions> => {
    const actions = config.actions ? config.actions(set, get) : ({} as Actions)
    // Per-action lazy dispatchers: each loads its own chunk on first call
    // (cached thereafter) and exposes `.preload()` to warm it early.
    const lazyDispatchers: Record<string, any> = {}
    if (config.lazyActions) {
      for (const key of Object.keys(config.lazyActions)) {
        const loader = (config.lazyActions as LazyActionsConfig<State>)[key]
        let implPromise: Promise<(...args: any[]) => any> | null = null
        const resolveImpl = () =>
          (implPromise ??= loader().then(m => m.default(set, get)))
        const dispatch = (...args: any[]) =>
          resolveImpl().then(impl => impl(...args))
        // preload: warm the chunk + build the impl, but do not invoke it.
        dispatch.preload = () => resolveImpl().then(() => undefined)
        lazyDispatchers[key] = dispatch
      }
    }
    const cleanups: Unsubscribe[] = []
    const ctx: StoreInitCtx<State> & { actions: Actions } = {
      set,
      get,
      // init sees eager actions AND the lazy dispatchers (so `init` can kick off
      // a lazy loader on first access — it loads that action's chunk on demand).
      actions: { ...actions, ...lazyDispatchers } as Actions,
      on: (event, handler) => {
        const busOn = useEventBusStore.getState().on as (
          e: string,
          h: EventHandler<any>,
          g?: string,
        ) => Unsubscribe
        cleanups.push(busOn(event as string, handler as EventHandler<any>, groupName))
      },
      watch: (store, selector, cb, opts) => {
        cleanups.push(
          (store.subscribe as any)(selector, cb, { fireImmediately: true, ...opts }),
        )
      },
      onCleanup: fn => {
        cleanups.push(fn)
      },
    }
    return {
      ...(config.state as State),
      ...actions,
      ...lazyDispatchers,
      __init__: {
        __store__: () => {
          config.init?.(ctx)
          // Auto-warm this store's lazy-action chunks on idle — every store's
          // interactions become instant with ZERO per-store preload boilerplate.
          autoWarmLazyActions(lazyDispatchers)
        },
      },
      __destroy__: () => {
        cleanups.splice(0).forEach(off => {
          try {
            off()
          } catch {
            /* ignore */
          }
        })
        useEventBusStore.getState().removeGroupListeners(groupName)
      },
    } as FullStoreState<State, Actions>
  }
}

/** Apply the middleware stack in the fixed order the kit guarantees:
 *  `subscribeWithSelector( persist?( immer?( builder ) ) )`. subscribeWithSelector
 *  stays OUTERMOST so `store.subscribe(selector, cb, opts)` type-checks; persist
 *  (when configured) wraps the base creator so only `partialize`'d keys persist;
 *  immer (when enabled) is innermost so draft setters work under both. */
function applyMiddleware<State extends object, Actions extends object>(
  builder: (set: any, get: any) => FullStoreState<State, Actions>,
  config: StoreConfig<State, Actions>,
) {
  const withImmer = config.immer ? immerMiddleware(builder as any) : builder
  const withPersist = config.persist
    ? persistMiddleware(withImmer as any, config.persist)
    : withImmer
  return subscribeWithSelector(withPersist as any)
}

/** The folder-glob store config: `actions` is `import.meta.glob('./actions/*.ts')`;
 *  `AM` is the generated `actions.gen.ts` type map (name → factory type). */
export interface GlobStoreConfig<State extends object, AM> {
  actions: ActionGlob
  immer?: boolean
  persist?: PersistOptions<State, any>
  state: State
  init?: (
    ctx: StoreInitCtx<State> & { actions: DispatchersFromTypeMap<AM> },
  ) => void
}

/** The EAGER folder-glob config: `actions: import.meta.glob('./actions/*.ts',
 *  { eager: true })` — same folder structure as `GlobStoreConfig`, but actions
 *  load synchronously so a store with a synchronous selector keeps its sync
 *  signature. `AM` is still the generated `actions.gen.ts` type map. */
export interface EagerGlobStoreConfig<State extends object, AM> {
  actions: EagerActionGlob
  immer?: boolean
  persist?: PersistOptions<State, any>
  state: State
  init?: (
    ctx: StoreInitCtx<State> & { actions: EagerDispatchersFromTypeMap<AM> },
  ) => void
}

/**
 * Global singleton store. Register it on a module via
 * `createModule({ stores: [MyStore] })` — the name is written ONCE here, and
 * consumers read it through `Stores.<name>` exactly as before.
 *
 * Two forms:
 *  1. Explicit `actions`/`lazyActions` (legacy).
 *  2. Folder-glob auto-registration: `actions: import.meta.glob('./actions/*.ts')`
 *     — store-kit registers each file by basename; types come from the `AM`
 *     generic (the generated `actions.gen.ts` map).
 */
export function defineStore<
  State extends object,
  Actions extends object = {},
  LA extends LazyActionsConfig<State> = {},
>(
  name: string,
  config: StoreConfig<State, Actions, LA>,
): StoreHandle<FullStoreState<State, Actions & LazyDispatchers<LA>>>
export function defineStore<State extends object, AM extends Record<string, any>>(
  name: string,
  config: GlobStoreConfig<State, AM>,
): StoreHandle<FullStoreState<State, DispatchersFromTypeMap<AM>>>
export function defineStore<State extends object, AM extends Record<string, any>>(
  name: string,
  config: EagerGlobStoreConfig<State, AM>,
): StoreHandle<FullStoreState<State, EagerDispatchersFromTypeMap<AM>>>
export function defineStore(name: string, config: any): any {
  const normalized = normalizeGlobConfig(config) as StoreConfig<any, any>
  const builder = makeBuilder(name, normalized)
  const store = create<any>()(
    applyMiddleware(builder as any, normalized) as any,
  ) as BoundStore<any>
  return { name, store }
}

// ============================================================================
// defineExtensionStore — chat-extension stores (the `Stores.Chat.<Name>` family).
//
// Same authoring model (state / actions / init / immer / persist / $) as
// defineStore, but returns a FACTORY (`() => proxy`) instead of a {name, store}
// handle — because the chat-extension registry calls `createStore()` per
// registration and INJECTS the returned proxy into the Chat store, so it's read
// nested as `Stores.Chat.<Name>` rather than top-level `Stores.<Name>`.
//
// The returned proxy is the SAME `createStoreProxy` used everywhere, so reads
// are identical (`const {a}=Stores.Chat.X`, `Stores.Chat.X.$.a`, actions), and
// the `init` listeners auto-tear-down via the proxy's ref-count destroy. Each
// `createStore()` call gets a fresh instance + a unique EventBus group so two
// live instances can't clobber each other's listeners (like defineLocalStore).
//
//   export const createTextStore = defineExtensionStore({
//     immer: true,
//     state: { draft: '' },
//     actions: (set) => ({ setDraft: (v: string) => set(s => { s.draft = v }) }),
//   })
//   // extension.tsx →  store: { name: 'TextStore', createStore: createTextStore }
// ============================================================================
export function defineExtensionStore<State extends object, Actions extends object>(
  config: StoreConfig<State, Actions>,
): () => StoreProxy<FullStoreState<State, Actions>>
export function defineExtensionStore<
  State extends object,
  AM extends Record<string, any>,
>(
  config: GlobStoreConfig<State, AM>,
): () => StoreProxy<FullStoreState<State, DispatchersFromTypeMap<AM>>>
export function defineExtensionStore<
  State extends object,
  AM extends Record<string, any>,
>(
  config: EagerGlobStoreConfig<State, AM>,
): () => StoreProxy<FullStoreState<State, EagerDispatchersFromTypeMap<AM>>>
export function defineExtensionStore(configArg: any): any {
  // Accept the folder-glob form (`actions: import.meta.glob('./actions/*.ts')`)
  // identically to defineStore/defineLocalStore — normalize it to lazyActions
  // before building so extension stores share the ONE authoring pattern.
  const config = normalizeGlobConfig(configArg) as StoreConfig<any, any>
  let counter = 0
  return () => {
    const group = `chat-ext:${counter++}`
    const builder = makeBuilder(group, config)
    const store = create<FullStoreState<any, any>>()(
      applyMiddleware(builder, config) as any,
    ) as BoundStore<FullStoreState<any, any>>
    return createStoreProxy(store)
  }
}

// ============================================================================
// defineLocalStore — PRIVATE, per-component-instance stores.
//
// Same authoring model (state / actions / init / immer / $) as defineStore, but:
//   - NOT registered in Stores.X (each mount gets its own instance)
//   - `init` runs on MOUNT; every `on`/`watch` auto-unsubscribes on UNMOUNT
//     (a plain useEffect cleanup — no ref-count / delayed-destroy guessing).
//   - reactive reads keep the same shape: `const { a, b } = MyDef.use()`.
//
//   const Filter = defineLocalStore({
//     immer: true,
//     state: { query: '' },
//     actions: (set) => ({ setQuery: (q: string) => set(d => { d.query = q }) }),
//     init: ({ on }) => { on('sync:tags', () => {}) },  // torn down on unmount
//   })
//   function Panel() {
//     const s = Filter.use({ query: 'x' })   // fresh per mount
//     const { query } = s                     // reactive (same clean syntax)
//     return <input value={query} onChange={e => s.setQuery(e.target.value)} />
//   }
// ============================================================================

/** A per-instance store handle: reactive reads (`const {a}=s`), `s.$.a` snapshot,
 *  and `s.action()`. */
export type LocalStoreInstance<FullState> = Readonly<
  FullState & { $: FullState; __api__: StoreApi<FullState> }
>

export interface LocalStoreDef<FullState> {
  /** Instantiate a fresh store for THIS component (initial-state override
   *  merged in). Call it in render like any hook. */
  use: (initial?: Partial<FullState>) => LocalStoreInstance<FullState>
}

function createLocalProxy<S extends object>(
  api: StoreApi<S>,
): LocalStoreInstance<S> {
  return new Proxy({} as LocalStoreInstance<S>, {
    get: (_, prop) => {
      // `$` is the sole hook-free snapshot escape (state reads in handlers);
      // the `__state` alias was removed. Actions (function-valued props) are
      // returned resolved from getState(), hook-free — callable in render AND
      // handlers with no `$`.
      if (prop === '$') return api.getState()
      if (prop === '__api__') return api
      const value = (api.getState() as any)[prop]
      if (typeof value === 'function') return value
      // eslint-disable-next-line react-hooks/rules-of-hooks
      return useStore(api, useShallow((s: any) => s[prop]))
    },
  })
}

export function defineLocalStore<
  State extends object,
  Actions extends object = {},
  LA extends LazyActionsConfig<State> = {},
>(
  config: StoreConfig<State, Actions, LA>,
): LocalStoreDef<FullStoreState<State, Actions & LazyDispatchers<LA>>>
export function defineLocalStore<
  State extends object,
  AM extends Record<string, any>,
>(
  config: GlobStoreConfig<State, AM>,
): LocalStoreDef<FullStoreState<State, DispatchersFromTypeMap<AM>>>
export function defineLocalStore<
  State extends object,
  AM extends Record<string, any>,
>(
  config: EagerGlobStoreConfig<State, AM>,
): LocalStoreDef<FullStoreState<State, EagerDispatchersFromTypeMap<AM>>>
export function defineLocalStore(configArg: any): any {
  // A distinct EventBus group per live instance so instances don't clobber each
  // other's listeners (defineStore's global variant can key by the store name;
  // locals can't). The runtime `makeBuilder` already spreads the lazy-action
  // dispatchers into state; the LA/AM generic just surfaces them in the TYPE (so
  // a per-pane store — e.g. Chat — exposes its lazy actions to consumers).
  const config = normalizeGlobConfig(configArg) as StoreConfig<any, any>
  type Full = any
  let counter = 0
  return {
    use: (initial: any) => {
      const ref = useRef<{
        api: StoreApi<Full>
        proxy: LocalStoreInstance<Full>
      } | null>(null)

      if (ref.current === null) {
        const group = `local:${counter++}`
        const merged = initial
          ? { ...config, state: { ...config.state, ...initial } }
          : config
        const builder = makeBuilder(group, merged as StoreConfig<any, any>)
        const api = createStore<Full>()(
          applyMiddleware(builder as any, merged as StoreConfig<any, any>) as any,
        ) as StoreApi<Full>
        ref.current = { api, proxy: createLocalProxy(api) }
      }

      const { api, proxy } = ref.current
      // init on mount, teardown on unmount — lifecycle rides the component.
      useEffect(() => {
        api.getState().__init__.__store__()
        // `__destroy__` may be async (a store whose teardown awaits a lazy
        // action); the React cleanup fires-and-forgets it (must return void).
        return () => {
          void api.getState().__destroy__()
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [])
      return proxy
    },
  }
}

// Whole-store-lazy registration (defined in ./stores; re-exported for co-location with defineStore).
export { registerLazyStore } from './stores'
