import { create, useStore } from 'zustand'
import { createStore } from 'zustand/vanilla'
import {
  createJSONStorage,
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
import { createStoreProxy, type StoreProxy, type StoreVisibility } from './stores'
import { onNetworkIdle } from './net-idle'
import { getWarmScheduler } from './warm-scheduler'
import { createLazyActionSequencer, createLazyDispatcher } from './lazy-dispatch'
import { isStaleBuild } from './chunk-recovery'
import {
  applyStoreSeed,
  createServerPersistStorage,
  isStoreServerRender,
  registerStoreExpose,
  type StoreExposeOption,
  type StoreSeedOption,
} from './store-kit-seed'

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

// ============================================================================
// Quiet while hidden — the store half of `pinStore` (see stores.ts, "Pins").
//
// A store that an owner PINS while nothing renders it is QUIET: the page that
// shows it is not on screen, but the app wants it kept (it is one Back away).
// A quiet store must not spend the network on a page nobody sees, so:
//
//   - every `on(event, …)` handler is DEFERRED while quiet — the event is
//     queued (deduped by event + payload, oldest first), never run;
//   - every `watch(…)` callback is likewise parked (first `prev`, latest
//     value — replayed only when the value really moved);
//   - when the store is SHOWN again the queue is replayed, in order, once;
//   - and when it was hidden for at least the app's revalidate window
//     (`setQuietPolicy`) — or it was RELEASED while hidden with events still
//     queued — the store's own `sync:reconnect` handler runs once, as the
//     revalidation (the "refresh in place" every store already answers), and
//     any queued `sync:*` frame it supersedes is dropped.
//
// A handler that must run even while hidden opts out with
// `on(event, h, { whileQuiet: 'run' })`. `ctx.isQuiet()` lets a timer skip
// its ticks, and `ctx.onWake(fn)` lets a store revalidate in its own way.
// A store nothing pins is never quiet — every other app is byte-identical.
// ============================================================================

/** How a quiet store's handler treats an event that arrives while hidden. */
export interface QuietOption {
  /** `'defer'` (default): queue it until the store is shown again.
   *  `'run'`: run it now, hidden or not. */
  whileQuiet?: 'defer' | 'run'
  /** `sync:reconnect` only — `false` keeps this handler out of the wake
   *  revalidation (a store whose reconnect answer is DESTRUCTIVE — it drops
   *  what is on screen — revalidates in its own `onWake` instead). */
  wakeRevalidate?: boolean
}

/** What a waking store is told (`ctx.onWake`). */
export interface StoreWakeInfo {
  /** How long the store was quiet (ms). */
  hiddenMs: number
  /** Whether the app's revalidate window passed (or events were lost on a
   *  release) — the store should revalidate. */
  stale: boolean
  /** How many deferred events/watch callbacks were replayed. */
  replayed: number
}

/** The app-wide quiet policy. `revalidateAfterMs` = how long a store may stay
 *  hidden before it revalidates on its return (default: never). */
export interface QuietPolicy {
  revalidateAfterMs: number
}

const quietPolicy: QuietPolicy = { revalidateAfterMs: Number.POSITIVE_INFINITY }

/** Set the app-wide quiet policy (call once at boot). */
export function setQuietPolicy(policy: Partial<QuietPolicy>): void {
  if (typeof policy.revalidateAfterMs === 'number' && policy.revalidateAfterMs >= 0) {
    quietPolicy.revalidateAfterMs = policy.revalidateAfterMs
  }
}

/** The current quiet policy (a copy). */
export function getQuietPolicy(): QuietPolicy {
  return { ...quietPolicy }
}

/** At most this many deferred items per store; past it the queue is folded
 *  into one revalidation (a store hidden through a storm of frames refetches
 *  once instead of replaying the storm). */
const QUIET_QUEUE_MAX = 64

/** The event whose handler is a store's "refresh in place" (the SDK sync
 *  client emits it on a genuine reconnect; every syncing store answers it). */
const REVALIDATE_EVENT = 'sync:reconnect'

/** Base toolkit passed to `init` (plus `actions`, added in StoreConfig). */
export interface StoreInitCtx<State> {
  set: StoreSet<State>
  get: () => State
  /** Subscribe to an app event (auto-grouped by the store name, auto-cleaned).
   *  Deferred while the store is quiet unless `{ whileQuiet: 'run' }`. */
  on: <K extends keyof AppEvents>(
    event: K,
    handler: EventHandler<AppEvents[K]>,
    opts?: QuietOption,
  ) => void
  /** React to another store's slice (replaces raw `useX.subscribe`, auto-cleaned).
   *  Fires immediately by default. Parked while quiet unless
   *  `{ whileQuiet: 'run' }`. */
  watch: <S, U>(
    store: Subscribable<S>,
    selector: (s: S) => U,
    cb: (value: U, prev: U) => void,
    opts?: { fireImmediately?: boolean; equalityFn?: (a: U, b: U) => boolean } & QuietOption,
  ) => void
  /** Register an arbitrary teardown to run on store destroy — the escape hatch
   *  for imperative resources (an SSE AbortController, a timer) that aren't an
   *  `on`/`watch` subscription. Runs alongside the auto-cleaned listeners. */
  onCleanup: (fn: () => void) => void
  /** Is the store quiet right now (pinned, nothing renders it)? A timer or a
   *  poll checks this and skips its tick. */
  isQuiet: () => boolean
  /** Run `fn` each time the store is shown again after being quiet (after the
   *  deferred queue replayed). Auto-cleaned on destroy. */
  onWake: (fn: (info: StoreWakeInfo) => void) => void
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

/**
 * Compile-time prefetch toggle. The baked-in auto-warm (preload every store's
 * lazy action chunks on init) is ON by default. Build with
 * `VITE_STORE_PREFETCH=off` to strip it — Vite inlines `import.meta.env.VITE_*`
 * to a literal, so `STORE_PREFETCH_ENABLED` const-folds to `false` and the warm
 * loop is dead-code-eliminated. Lets you measure a COLD page render with ZERO
 * action chunks prefetched (only the code the first paint actually needs).
 * Read as a plain `import.meta.env.VITE_*` (no `?.`) — same pattern as the
 * framework's existing `import.meta.env.DEV` reads — so Vite const-folds it to a
 * literal and the warm loop below dead-code-eliminates when off.
 */
const STORE_PREFETCH_ENABLED = ((): boolean => {
  try {
    return import.meta.env.VITE_STORE_PREFETCH !== 'off'
  } catch {
    // `import.meta.env` is injected by the BUNDLER and is undefined under a
    // plain node runtime — so the read above throws a TypeError at MODULE
    // SCOPE, taking down anything that merely imports this file outside a
    // bundle. That is not hypothetical: a consuming app's `collect-actions` /
    // `action-refs-lint` / `actions-unit` tooling esbuild-bundles app sources
    // and runs them on `platform: 'node'`; the moment an app store reached
    // store-kit, `npm run check` died with no useful message. Its neighbour
    // `module-system/store.ts::isDev()` already guards the identical hazard.
    //
    // The try/catch does NOT defeat the dead-code elimination the plain read
    // exists for: under Vite the expression const-folds to a literal BEFORE
    // minification (`'off' !== 'off'` → `false`), the IIFE inlines, and the
    // warm loop below is eliminated exactly as before. Under node there are no
    // chunks to warm and no network-idle to wait for, so `false` is also the
    // behaviourally correct answer rather than merely a safe one.
    return false
  }
})()

/** BAKED-IN PREFETCH: on init, warm every lazy-action chunk on idle so the store's
 *  interactions are instant — with NO per-store `.preload()` wiring. Actions already
 *  invoked in `init` (hot) are a cached no-op; the rest fetch in the background.
 *  Gated by the `VITE_STORE_PREFETCH=off` compile-time flag (see above). */
function autoWarmLazyActions(
  storeName: string,
  lazyDispatchers: Record<string, any>,
  enabled: boolean = STORE_PREFETCH_ENABLED,
): void {
  if (!enabled) return
  const keys = Object.keys(lazyDispatchers)
  if (!keys.length) return
  // An app-installed scheduler owns the timing, the cap and the order
  // (`warm-scheduler.ts`): hand it one task per action and stop. The task
  // re-checks the stale-build mark when it actually RUNS, which may be long
  // after this init.
  const scheduler = getWarmScheduler()
  if (scheduler) {
    for (const k of keys) {
      scheduler(
        () => {
          if (isStaleBuild()) return undefined
          return lazyDispatchers[k].preload?.()
        },
        { source: 'store-actions', key: `${storeName}.${k}` },
      )
    }
    return
  }
  // Wait for the page's CRITICAL api-client calls to finish (network-idle after
  // load) BEFORE warming, so the prefetch never competes with them for the
  // browser's connections — THEN schedule the actual preloads on a main-thread
  // idle gap. (Fixes: on /login the drawer/onboarding chunks were prefetched
  // while the auth/providers call was still pending.)
  onNetworkIdle(() =>
    warmIdle(() => {
      for (const k of keys) {
        // Skip warming once chunk loading is known to be broken.
        //
        // HONEST SCOPE, because an earlier comment here overstated it: this loop
        // body is SYNCHRONOUS (`preload()` is invoked, not awaited) and the mark
        // can only be set asynchronously, so it cannot flip BETWEEN iterations —
        // the store whose chunks are failing still fires all of its own keys.
        // What this does prevent is every store scheduled on a LATER idle tick
        // (route entry, lazy module registration) from repeating the exercise, so
        // one deploy-while-a-tab-is-open costs one store's keys rather than every
        // registered store's. The on-demand dispatch is unaffected: it still
        // tries, and reports. A successful import clears the mark, so a transient
        // blip does not disable prefetch for the session.
        if (isStaleBuild()) return
        try {
          // `.catch` is REQUIRED, not belt-and-braces: `preload()` returns a
          // promise, so the surrounding try/catch only ever caught a synchronous
          // throw. A chunk that fails to load therefore produced an UNHANDLED
          // REJECTION — i.e. an uncaught page-level error — for a warm-up nobody
          // asked for and nothing awaits. (Observed directly: blocking one action
          // chunk in an e2e produced page errors from this loop alone.)
          //
          // A failed warm-up is a genuine no-op: the chunk is simply not warm, and
          // the on-demand dispatch retries it and surfaces a real error to the
          // caller if it still fails. So swallow it at debug volume — the
          // `vite:preloadError` listener has already logged the load failure once
          // and marked the build stale.
          lazyDispatchers[k].preload?.().catch((err: unknown) => {
            console.debug(`[store-kit] prefetch of lazy action "${k}" failed (ignored)`, err)
          })
        } catch (err) {
          console.debug(`[store-kit] prefetch of lazy action "${k}" threw (ignored)`, err)
        }
      }
    }),
  )
}

/** TEST SEAM ONLY — the warm with the compile-time flag forced on (under
 *  `node --test` the flag is `false`: there is no bundler to inline it). */
export function __autoWarmLazyActionsForTests(
  storeName: string,
  lazyDispatchers: Record<string, any>,
): void {
  autoWarmLazyActions(storeName, lazyDispatchers, true)
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
  /** SSR: merge this store's slice from the incoming document seed into its
   *  BIRTH state (see `store-kit-seed.ts`). `true` = shallow spread over
   *  `state`. Changing birth state rather than `setState`-ing after the fact is
   *  what makes a seeded store need no zustand patching on a hydrating client:
   *  `getInitialState()` — which is what React reads as the server snapshot —
   *  already returns the seeded value. */
  seeded?: StoreSeedOption<State>
  /** SSR: what of this store travels to the next document. `true` = the whole
   *  state object. Registered as a GETTER at definition time and read once, on
   *  a server, after the render has quiesced. */
  ssrExpose?: StoreExposeOption<State>
}

/** Internal lifecycle keys the Stores proxy already understands. */
interface Lifecycle {
  __init__: { __store__: () => void }
  /** The proxy's visibility facts (render refs / owner pins) — see stores.ts. */
  __visibility__: (v: StoreVisibility) => void
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

/** A deferred event's dedupe key component: the payload, serialized when it
 *  can be (two identical sync frames collapse into one replay). */
function payloadKey(data: unknown): string {
  if (data === undefined) return ''
  try {
    return JSON.stringify(data) ?? ''
  } catch {
    return '?'
  }
}

/** Run one replayed handler the way the EventBus runs it: a throw or a
 *  rejection is logged and never escapes into the render that woke it. */
function runIsolated(fn: () => unknown, what: string): void {
  try {
    const out = fn()
    if (out && typeof (out as Promise<unknown>).then === 'function') {
      ;(out as Promise<unknown>).catch(err => console.error(`[store-kit] ${what} failed`, err))
    }
  } catch (err) {
    console.error(`[store-kit] ${what} failed`, err)
  }
}

interface DeferredItem {
  key: string
  /** The event name (`null` for a parked `watch`). */
  event: string | null
  run: () => void
}

/** One store's quiet bookkeeping (see "Quiet while hidden" above). */
function createQuietState() {
  const st = {
    quiet: false,
    visible: false,
    /** When the store went quiet (null = not hidden-while-pinned). */
    hiddenSince: null as number | null,
    /** Events were lost (queue overflow, or released while hidden). */
    staleOnShow: false,
    queue: [] as DeferredItem[],
    handlers: new Map<string, EventHandler<any>>(),
    wakeFns: [] as Array<(info: StoreWakeInfo) => void>,
    nextWatchId: 0,
    defer(key: string, event: string | null, run: () => void): void {
      const at = st.queue.findIndex(q => q.key === key)
      if (at >= 0) st.queue.splice(at, 1)
      st.queue.push({ key, event, run })
      if (st.queue.length > QUIET_QUEUE_MAX) st.fold()
    },
    unqueue(key: string): void {
      const at = st.queue.findIndex(q => q.key === key)
      if (at >= 0) st.queue.splice(at, 1)
    },
    /** Fold the queued EVENTS into one revalidation-on-show. Parked `watch`
     *  callbacks are kept: they carry a value change no revalidation replays
     *  (an auth/permission edge), and each is one entry, deduped by key. */
    fold(): void {
      const before = st.queue.length
      st.queue = st.queue.filter(q => q.event === null)
      if (st.queue.length !== before) st.staleOnShow = true
    },
    visibility(v: StoreVisibility): void {
      const wasVisible = st.visible
      const wasQuiet = st.quiet
      st.visible = v.visible
      st.quiet = v.pinned && !v.visible
      if (!wasQuiet && st.quiet) {
        st.hiddenSince = Date.now()
        return
      }
      if (wasQuiet && !st.quiet && !v.visible) {
        // RELEASED while hidden (the last pin let go): nothing is deferred
        // any more, so what was queued is folded into one revalidation for
        // the moment the store is shown again (or dropped by its destroy).
        st.fold()
        return
      }
      if (!wasVisible && v.visible) st.wake()
    },
    wake(): void {
      // Shown for the first time, or shown again after an UNPINNED absence
      // (the ordinary destroy grace): nothing was deferred, nothing to do.
      if (st.hiddenSince === null && !st.staleOnShow && st.queue.length === 0) return
      const now = Date.now()
      const hiddenMs = st.hiddenSince === null ? 0 : now - st.hiddenSince
      const windowPassed = st.hiddenSince !== null && hiddenMs >= quietPolicy.revalidateAfterMs
      const stale = st.staleOnShow || windowPassed
      st.hiddenSince = null
      st.staleOnShow = false
      const revalidate = stale ? st.handlers.get(REVALIDATE_EVENT) : undefined
      // A revalidation supersedes every queued sync frame (it refetches all
      // of them); everything else replays in arrival order.
      const queue = st.queue
        .splice(0)
        .filter(q => !(revalidate && q.event !== null && q.event.startsWith('sync:')))
      for (const item of queue) runIsolated(item.run, 'deferred handler')
      if (revalidate) {
        runIsolated(
          () => revalidate({ type: REVALIDATE_EVENT, data: {}, timestamp: now } as never),
          'revalidation',
        )
      }
      if (st.wakeFns.length === 0) return
      const info: StoreWakeInfo = { hiddenMs, stale, replayed: queue.length }
      for (const fn of st.wakeFns) runIsolated(() => fn(info), 'onWake')
    },
    reset(): void {
      st.quiet = false
      st.visible = false
      st.hiddenSince = null
      st.staleOnShow = false
      st.queue.length = 0
      st.handlers.clear()
      st.wakeFns.length = 0
    },
  }
  return st
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
      // ONE call-order sequencer per store instance, shared by every lazy
      // action's dispatcher (#659): with per-action chunks, the old
      // `resolveImpl().then(impl => impl(...args))` ran each impl in
      // CHUNK-LOAD order, so `X.edit(patch); X.save()` could run `save` before
      // `edit` and write the previous draft (the production kid-mode bug).
      // See `LazyActionSequencer` (lazy-dispatch.ts) for the guarantee's
      // exact wording; in short: on ONE store, lazy impls are INVOKED in
      // dispatch-call order — by START, never by SETTLE, so slow actions do
      // not serialize later ones and an action may await another action on
      // its own store. Different stores never share a sequencer.
      const sequencer = createLazyActionSequencer()
      for (const key of Object.keys(config.lazyActions)) {
        const loader = (config.lazyActions as LazyActionsConfig<State>)[key]
        // The dispatcher (chunk memoization, `.preload()`, the chunk-load
        // de-dup window that keeps each action's OWN in-flight guard reachable,
        // and the per-store call-order sequencer) lives in ./lazy-dispatch.ts —
        // see the rationale there.
        //
        // The two stages are passed SEPARATELY on purpose: the dispatcher must
        // distinguish a TRANSIENT chunk-download failure (retry, never memoize —
        // a deploy while the tab is open invalidates every hashed chunk URL)
        // from a DETERMINISTIC action-factory throw (memoize, fail fast).
        lazyDispatchers[key] = createLazyDispatcher(
          () => loader(),
          m => m.default(set, get),
          { sequencer },
        )
      }
    }
    const cleanups: Unsubscribe[] = []
    const quiet = createQuietState()
    const ctx: StoreInitCtx<State> & { actions: Actions } = {
      set,
      get,
      // init sees eager actions AND the lazy dispatchers (so `init` can kick off
      // a lazy loader on first access — it loads that action's chunk on demand).
      actions: { ...actions, ...lazyDispatchers } as Actions,
      on: (event, handler, opts) => {
        const busOn = useEventBusStore.getState().on as (
          e: string,
          h: EventHandler<any>,
          g?: string,
        ) => Unsubscribe
        const raw = handler as EventHandler<any>
        const name = event as string
        // The bus dedupes per `group::event`, so this store has ONE handler per
        // event — the one the revalidation calls (unless it opted out).
        if (opts?.wakeRevalidate === false) quiet.handlers.delete(name)
        else quiet.handlers.set(name, raw)
        const runQuiet = opts?.whileQuiet === 'run'
        const wrapped: EventHandler<any> = ev => {
          if (quiet.quiet && !runQuiet) {
            quiet.defer(`${name}|${payloadKey((ev as { data?: unknown } | undefined)?.data)}`, name, () =>
              raw(ev),
            )
            return
          }
          return raw(ev)
        }
        cleanups.push(busOn(name, wrapped, groupName))
      },
      watch: (store, selector, cb, opts) => {
        const runQuiet = opts?.whileQuiet === 'run'
        const eq = opts?.equalityFn ?? Object.is
        const key = `watch|${quiet.nextWatchId++}`
        let parked: { value: any; prev: any } | null = null
        const wrapped = (value: any, prev: any) => {
          if (quiet.quiet && !runQuiet) {
            parked = parked ? { value, prev: parked.prev } : { value, prev }
            quiet.defer(key, null, () => {
              const p = parked
              parked = null
              if (p && !eq(p.value, p.prev)) cb(p.value, p.prev)
            })
            return
          }
          // Not quiet any more (released while hidden): the parked OLDER edge
          // is folded into this newer one — never replayed after it — and the
          // consumer sees one edge from the value it last OBSERVED (A→B parked,
          // then B→C runs as A→C; a return to A is no edge at all).
          if (parked) {
            const from = parked.prev
            parked = null
            quiet.unqueue(key)
            if (!eq(value, from)) cb(value, from)
            return
          }
          cb(value, prev)
        }
        // zustand's own options only (`whileQuiet` is the kit's, not the store's).
        const subscribeOpts: { fireImmediately: boolean; equalityFn?: (a: any, b: any) => boolean } = {
          fireImmediately: opts?.fireImmediately ?? true,
        }
        if (opts?.equalityFn) subscribeOpts.equalityFn = opts.equalityFn
        cleanups.push((store.subscribe as any)(selector, wrapped, subscribeOpts))
      },
      onCleanup: fn => {
        cleanups.push(fn)
      },
      isQuiet: () => quiet.quiet,
      onWake: fn => {
        quiet.wakeFns.push(fn)
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
          autoWarmLazyActions(groupName, lazyDispatchers)
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
        // A destroyed store re-runs `init` on its next access: nothing deferred
        // survives (the re-init's own load is the fresh answer).
        quiet.reset()
      },
      __visibility__: (v: StoreVisibility) => {
        quiet.visibility(v)
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
    ? persistMiddleware(withImmer as any, serverSafePersist(config.persist))
    : withImmer
  return subscribeWithSelector(withPersist as any)
}

/**
 * On a server, replace the store's `persist` storage with a fresh per-render
 * one — OVERRIDING whatever it declared, never merely filling an absent slot.
 *
 * The spread order below is the whole rule: `...options` LAST would let a
 * store's own `storage` win, and two of comic's five persisted stores ship one
 * whose server fallback is a module-scope `Map` — i.e. exactly the value that
 * must not survive a request. So the override is written after the spread, and
 * `store-kit-seed.ts` carries why that is safe independently of the host's
 * isolation model.
 *
 * In a browser this returns the caller's own object, unchanged and un-copied.
 */
function serverSafePersist<State extends object>(
  options: PersistOptions<State, any>,
): PersistOptions<State, any> {
  if (!isStoreServerRender()) return options
  return {
    ...options,
    storage: createJSONStorage(() => createServerPersistStorage()),
  }
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
  /** SSR — see {@link StoreConfig.seeded}. Declared on all three overload
   *  shapes deliberately: 61 of comic's 66 stores are folder-glob, so a pair of
   *  options that existed only on the explicit-`actions` shape would be
   *  unreachable for almost every store that needs them. */
  seeded?: StoreSeedOption<State>
  /** SSR — see {@link StoreConfig.ssrExpose}. */
  ssrExpose?: StoreExposeOption<State>
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
  /** SSR — see {@link StoreConfig.seeded}. Declared on all three overload
   *  shapes deliberately: 61 of comic's 66 stores are folder-glob, so a pair of
   *  options that existed only on the explicit-`actions` shape would be
   *  unreachable for almost every store that needs them. */
  seeded?: StoreSeedOption<State>
  /** SSR — see {@link StoreConfig.ssrExpose}. */
  ssrExpose?: StoreExposeOption<State>
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
  // The seed is applied to the BIRTH state, before construction — not after,
  // and not through `setState`. `zustand/vanilla` captures `initialState` once
  // (`vanilla.mjs:13`) and `setState` never touches it, so a store seeded after
  // construction would render from its seed and HYDRATE from its defaults.
  const seeded = {
    ...normalized,
    state: applyStoreSeed(name, normalized.state, normalized.seeded),
  }
  const builder = makeBuilder(name, seeded)
  const store = create<any>()(
    applyMiddleware(builder as any, seeded) as any,
  ) as BoundStore<any>
  if (seeded.ssrExpose) {
    const select = seeded.ssrExpose
    registerStoreExpose(name, () =>
      typeof select === 'function' ? select(store.getState()) : plainState(store.getState()),
    )
  }
  return { name, store }
}

/** A store's DATA fields — the state object minus actions and the two lifecycle
 *  keys the builder always adds back. `ssrExpose: true` means "my data", and a
 *  function or a lifecycle hook in a JSON payload is a defect, not a slice. */
function plainState(full: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const [k, v] of Object.entries(full)) {
    if (k === '__init__' || k === '__destroy__') continue
    if (typeof v === 'function') continue
    out[k] = v
  }
  return out
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
