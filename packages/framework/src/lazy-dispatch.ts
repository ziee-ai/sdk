// ============================================================================
// Lazy-action dispatch — extracted from store-kit so it can be reasoned about
// (and unit-tested) on its own, with no zustand/EventBus dependency graph.
//
// A lazy action lives in its own chunk: calling it must (1) download the chunk,
// (2) build the impl from `(set, get)`, then (3) invoke it. The chunk + impl are
// memoized so the download happens once; `preload()` warms them without
// invoking the action.
//
// ── Why this module exists in two parts: dispatcher + sequencer ────────────
// Each lazy action has its OWN chunk, so with the naive dispatch
// (`resolveImpl().then(impl => impl(...args))`) two actions called
// back-to-back on one store ran in the order their chunks happened to resolve.
// #659 was exactly that, in production: `X.edit(patch); X.save()` could run
// `save` first and write the PREVIOUS draft. The fix keeps chunk loads
// parallel and serializes only each impl's INVOCATION, in CALL order, through
// one `LazyActionSequencer` per STORE (shared via `LazyDispatchOptions`).
//
// ── EXACT ORDERING GUARANTEE ───────────────────────────────────────────────
// On a single store (one sequencer):
//   1. Lazy-vs-lazy: if `X.a()` is dispatched before `X.b()` on the same
//      store, `a`'s impl is INVOKED before `b`'s. Always — cold chunks, warm
//      chunks, failures, everything. "Dispatched" = the call expression ran;
//      "invoked" = the action body started executing.
//   2. Call order, not settle order: the chain moves the moment an impl is
//      invoked (or its dispatch fails). A slow `a` never serializes later
//      actions behind its network call, and an action may dispatch and AWAIT
//      another action on its own store without deadlock.
//   3. Failure does not poison the store: an import failure or a synchronous
//      throw in one impl releases the chain; later dispatches still run.
//   4. Different stores have different sequencers — one store's slow chunk
//      never delays another store's actions.
//   5. `.preload()` never takes a chain slot (non-blocking, unsequenced).
//   6. Warm-path timing: a dispatch with no EARLIER dispatch still pending
//      (the common case — one action per handler) invokes on the chunk with
//      the pre-sequencing single-microtask timing; only back-to-back
//      dispatches pay the extra turn-wait hop, and that hop is the ONLY cost.
// Eager actions (inline `actions:` factories and `{ eager: true }` globs) are
// built as plain synchronous functions and are NOT sequenced: they run
// synchronously at their call site. Mixing is therefore deterministic in the
// only directions that matter — eager-vs-eager runs in call order
// (synchronous); an eager action called after a lazy action in the same tick
// still runs before that lazy action's (always-deferred) invocation; and
// lazy-vs-lazy keeps the guarantee above. A handler needing strict order
// ACROSS the two kinds must await the lazy call (as before — this is
// unchanged by the sequencer).
//
// ── Why the loader is passed as TWO stages ──────────────────────────────────
// The two stages fail for completely different reasons and must be recovered
// from differently:
//
//   - **import** (`() => import('./actions/foo')`) fails for TRANSIENT,
//     environmental reasons: a network blip, a proxy hiccup, or — the common one
//     in production — ANY deploy while a tab is open, which invalidates the
//     hashed chunk URLs the already-loaded page still holds. Retrying is
//     correct, and adding our OWN permanent memo on top is actively harmful.
//   - **build** (`mod => mod.default(set, get)`) fails only for DETERMINISTIC
//     reasons: the action factory itself threw, i.e. an authoring bug. Retrying
//     that forever would turn one bug into an unbounded loop for a component
//     that dispatches from a render or an effect.
//
// A single combined loader cannot tell the two apart (short of sniffing error
// messages, which is unreliable across browsers and bundlers), so the previous
// one-argument form conflated them and applied the DETERMINISTIC policy to both:
// one retry, then the rejection was memoized permanently. A live-UI audit caught
// the consequence — one transient blip permanently bricked the chat model
// picker's lazy action, and every subsequent send then posted a body missing
// `model_id` and got a raw 422.
//
// ── HONEST LIMIT: what the retry can and cannot recover ─────────────────────
// Retrying does NOT make every chunk failure self-heal in a browser, and this
// module must not pretend otherwise. Per the HTML spec the module map records a
// FAILED fetch for a URL, so re-`import()`ing the SAME bundler-rewritten
// specifier fails again without re-requesting, for the life of that document.
// Measured directly in this repo's e2e: 9 import attempts produced only 2
// network requests. A cache-busting specifier would defeat that, but the
// specifier is generated by the bundler and is not ours to alter here.
//
// What the retry DOES recover:
//   - a Vite `__vitePreload` DEP failure — the helper re-creates its
//     `<link rel=modulepreload>` on every call, so a link that 503s once can
//     succeed on the next attempt;
//   - any loader that is not a module-map-backed `import()` (a fetch-based
//     loader, a dev-server round trip, a test double);
//   - a failure that happens BEFORE the fetch is recorded (an aborted request
//     that never reached the map).
// What it does not recover — a hard 404/aborted fetch of a static specifier —
// is instead handled by TELLING THE USER to reload: the give-up path marks the
// build stale (below) and the caller renders an actionable message. Auto-reload
// was rejected deliberately: it destroys an unsent composer draft and tears down
// an in-flight stream.
//
// ── Why this does NOT de-duplicate calls ────────────────────────────────────
// There IS a real hole here: the action BODY — and therefore the action's own
// in-flight guard (`if (state.loading) return`) — cannot run until the chunk
// resolves, so two callers in the same tick both get past every guard. It is
// tempting to close that by merging same-argument calls made during the
// chunk-load window. That was tried and REJECTED, because a dispatcher cannot
// tell a read from a mutation:
//
//   - merging is applied to EVERY lazy action, so two deliberate identical
//     mutations issued in that window (a double-clicked create, two components
//     each dispatching `markRead(id)`) would collapse into one invocation and
//     BOTH callers would resolve successfully — a silently dropped intent;
//   - `JSON.stringify` does NOT throw on a function/Map/Set/class instance, it
//     emits `null`/`{}`, so calls carrying DIFFERENT callbacks key identically
//     and the second callback is never invoked.
//
// The duplicate NETWORK requests those un-guarded calls produce are removed one
// layer down, at the transport (`api-client/inflight.ts`), where "is this a
// read?" is knowable (`GET`, non-SSE, non-upload) and a mutation is excluded by
// construction. Two cold `loadConversations()` calls therefore still both run
// their body — harmless, they set the same state — but issue ONE request.
// ============================================================================

import { clearStaleBuild, markStaleBuild } from './chunk-recovery'

/** Stage 1: download the action's chunk. */
export type ModuleLoader<M> = () => Promise<M>

/** Stage 2: build the callable impl from the loaded module. */
export type ImplBuilder<M> = (mod: M) => (...args: any[]) => any

/** The callable a lazy action becomes on a store. Named for the ACTION it wraps
 *  (store-kit exports a differently-shaped, fully-typed `LazyDispatcher<L>` for
 *  the same concept at the type level — these must not share a name). */
export interface LazyActionDispatcher {
  (...args: any[]): Promise<any>
  /** Warm the chunk + build the impl without invoking it. */
  preload: () => Promise<void>
}

/**
 * How many times a failed impl BUILD is retried before the rejection is
 * memoized.
 *
 * A throw from the action FACTORY is a deterministic authoring bug — retrying it
 * forever would turn one bug into an unbounded loop for a component that
 * dispatches from a render or an effect. One retry covers a factory that
 * genuinely depended on a transient value; the second failure is treated as
 * deterministic and memoized, so the action fails fast and loudly from then on.
 */
const MAX_BUILD_RETRIES = 1

/**
 * How many EXTRA import attempts ONE dispatch makes before giving up.
 *
 * Bounded so a hard 404 (a chunk that is genuinely gone) still fails within a
 * few hundred milliseconds instead of hanging the caller. The rejection is never
 * memoized regardless of this budget, so a LATER dispatch always starts a fresh
 * one — that is what makes a transient blip self-healing rather than
 * session-fatal.
 */
const MAX_IMPORT_RETRIES = 2

/** Linear backoff between import attempts, in ms: 150 then 300. */
const IMPORT_RETRY_BACKOFF_MS = 150

const delay = (ms: number) =>
  new Promise<void>(resolve => setTimeout(resolve, ms))

export interface LazyDispatchOptions {
  /**
   * Sleep between import attempts. Defaults to a real `setTimeout`.
   *
   * Injectable purely as a TEST seam: the backoff constants are deliberately
   * tuned, and without this every failure-path spec would pay them in real wall
   * clock (~0.9-1.4s each) while asserting nothing about the timing.
   */
  sleep?: (ms: number) => Promise<void>
  /**
   * The store's call-order sequencer — ONE per store, shared by every lazy
   * action's dispatcher. See `createLazyActionSequencer` / `LazyDispatchOptions`.
   *
   * When omitted, the dispatcher sequences only ITS OWN calls (each dispatcher
   * gets a private sequencer); two dispatchers on the same store must share the
   * SAME sequencer for their invocations to be ordered against each other —
   * store-kit does exactly that (`store-kit.ts`'s `makeBuilder`).
   */
  sequencer?: LazyActionSequencer
}

/**
 * Per-store call-order sequencer for lazy actions (#659).
 *
 * Without it, each lazy action's impl runs in CHUNK-LOAD order — its own
 * chunk, its own `resolveImpl()` — so two actions called back-to-back on one
 * store (`X.edit(patch); X.save()`) race: with cold chunks `save` can run
 * before `edit` and write the previous draft. This was a real production bug
 * (the admin kid-mode switch saved ON after being clicked OFF).
 *
 * One sequencer instance is shared by EVERY lazy action of one store; dispatch
 * claims a chain slot in CALL order, and each slot's impl is not invoked until
 * every EARLIER slot's impl has been invoked (or that earlier dispatch has
 * failed). The guarantee is start order, NOT settle order: a slot releases the
 * chain the moment its impl is invoked (or its invocation failed), so a slow
 * action never serializes later ones behind its network call, and an action
 * that dispatches + awaits another action on its own store does not deadlock
 * (the awaited action's start does not wait for the caller's settle).
 */
export interface LazyActionSequencer {
  /**
   * Claim the next dispatch slot in call order. Synchronous — must be called
   * at dispatch time.
   *
   * @returns `turn` — a never-rejecting promise that resolves when every
   *   earlier slot's impl has been invoked or its dispatch has failed (i.e.
   *   when it is this slot's turn); `release` — call EXACTLY ONCE, at the
   *   moment this slot's impl is invoked or its dispatch gives up, so the next
   *   slot can proceed; `idle` — true when NO earlier slot is pending, in
   *   which case `turn` is already resolved and the dispatcher invokes with the
   *   pre-sequencing single-hop timing (the chain only costs microtasks when it
   *   actually has something to wait for).
   */
  claim(): { turn: Promise<void>; release: () => void; idle: boolean }
}

/** Build a per-store call-order sequencer (see `LazyActionSequencer`). */
export function createLazyActionSequencer(): LazyActionSequencer {
  let tail: Promise<void> = Promise.resolve()
  // Slots claimed but not yet released. `idle` (no earlier dispatch still
  // waiting for ITS turn) must count RELEASES against CLAIMS per slot, not a
  // single busy flag: clearing the flag on the first release would let a third
  // dispatch skip the wait while a second one is still between its turn and
  // its invocation.
  let pending = 0
  return {
    claim() {
      const idle = pending === 0
      pending++
      const turn = tail
      let release!: () => void
      tail = new Promise<void>(r => (release = r))
      return {
        turn,
        release: () => {
          pending--
          release()
        },
        idle,
      }
    },
  }
}

/**
 * Build the lazy dispatcher for ONE action.
 *
 * @param importModule stage 1 — download the chunk. A rejection here is treated
 *        as TRANSIENT: retried with backoff, and never memoized.
 * @param buildImpl    stage 2 — build the callable from the loaded module. A
 *        throw here is treated as DETERMINISTIC: memoized after
 *        `MAX_BUILD_RETRIES`.
 */
export function createLazyDispatcher<M = any>(
  importModule: ModuleLoader<M>,
  buildImpl: ImplBuilder<M>,
  options: LazyDispatchOptions = {},
): LazyActionDispatcher {
  let implPromise: Promise<(...args: any[]) => any> | null = null
  let buildFailures = 0
  const sleep = options.sleep ?? delay
  // Per-store call-order sequencer (#659). store-kit passes ONE shared
  // instance per store; a standalone dispatcher sequences its own calls.
  const sequencer = options.sequencer ?? createLazyActionSequencer()

  /** Import with bounded retry + linear backoff. Rejects with the LAST error. */
  const importWithRetry = async (): Promise<M> => {
    let lastError: unknown
    for (let attempt = 0; attempt <= MAX_IMPORT_RETRIES; attempt++) {
      if (attempt > 0) await sleep(IMPORT_RETRY_BACKOFF_MS * attempt)
      try {
        const mod = await importModule()
        // A NULLISH module namespace means the import FAILED but something
        // swallowed the rejection — Vite's `__vitePreload` does exactly this
        // when a `vite:preloadError` listener calls `preventDefault()`:
        // `baseModule().catch(handlePreloadError)` then resolves with
        // `undefined`. Left alone it reaches `buildImpl`, blows up with a
        // confusing "Cannot read properties of undefined (reading 'default')",
        // and — worse — is misclassified as a DETERMINISTIC factory bug and
        // memoized forever. Classify it here as what it is: a failed import.
        if (mod == null) {
          throw new Error(
            'lazy chunk import resolved with no module — the load failed and something suppressed the rejection (e.g. a vite:preloadError listener calling preventDefault)',
          )
        }
        // A chunk just loaded: whatever failed earlier is no longer failing, so
        // the stale mark (which gates both the user-facing "the app may have been
        // updated" hint and store-kit's prefetch bail) must not outlive it.
        clearStaleBuild()
        return mod
      } catch (err) {
        lastError = err
      }
    }
    throw lastError
  }

  const resolveImpl = (): Promise<(...args: any[]) => any> => {
    if (implPromise) return implPromise
    implPromise = (async () => {
      let mod: M
      try {
        mod = await importWithRetry()
      } catch (err) {
        // TRANSIENT — clear the memo unconditionally so the user's NEXT action
        // re-downloads the chunk. This dispatcher must never ADD a permanent
        // memo of its own on top of whatever the platform already caches.
        implPromise = null
        // Record that a code-split chunk has definitively failed in this page's
        // lifetime. Vite's `vite:preloadError` listener marks the same flag, but
        // it only fires for `__vitePreload`-wrapped imports in a production
        // build — this covers dev, a plain `import()`, and the `mod == null`
        // classification above, so the user-facing message can explain WHY
        // reloading helps in those cases too.
        markStaleBuild()
        throw err
      }
      try {
        return buildImpl(mod)
      } catch (err) {
        // DETERMINISTIC — one retry, then memoize, so an authoring bug fails
        // fast and loudly instead of looping.
        buildFailures++
        if (buildFailures <= MAX_BUILD_RETRIES) implPromise = null
        throw err
      }
    })()
    return implPromise
  }

  const dispatch = ((...args: any[]) => {
    // Claim this dispatch's slot in the store's call order BEFORE starting the
    // chunk load: both happen in the call's tick, so chunk loads stay fully
    // parallel across actions and only the IMPL INVOCATION is serialized.
    const { turn, release, idle } = sequencer.claim()
    const started = resolveImpl()
    if (idle) {
      // No earlier dispatch is pending — invoke on the chunk with the
      // pre-sequencing single-hop timing. `release` (in `finally`, and in the
      // rejection arm) still fires so a later domino of back-to-back dispatches
      // remains correctly ordered from HERE on.
      return started.then(
        impl => {
          try {
            return impl(...args)
          } finally {
            release()
          }
        },
        (err: unknown) => {
          release()
          throw err
        },
      )
    }
    return (async () => {
      // Wait for every earlier dispatch on the store to have STARTED (or
      // failed), never for them to settle (#659).
      await turn
      let result: any
      try {
        const impl = await started
        // Invoking the impl IS the start: release in `finally` so the chain
        // moves on the moment this impl is invoked — and ALSO when the import
        // fails (`await started` rejects) or the impl throws synchronously —
        // otherwise one failed action would block the store's chain forever.
        // An async impl's returned promise settles the caller's promise via
        // the async wrapper below WITHOUT holding the chain.
        result = impl(...args)
      } finally {
        release()
      }
      return result
    })()
  }) as LazyActionDispatcher

  dispatch.preload = () => resolveImpl().then(() => undefined)
  return dispatch
}
