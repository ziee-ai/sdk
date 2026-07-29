// ============================================================================
// Lazy-action dispatch — extracted from store-kit so it can be reasoned about
// (and unit-tested) on its own, with no zustand/EventBus dependency graph.
//
// A lazy action lives in its own chunk: calling it must (1) download the chunk,
// (2) build the impl from `(set, get)`, then (3) invoke it. The chunk + impl are
// memoized so the download happens once; `preload()` warms them without
// invoking the action.
//
// ── Why the loader is passed as TWO stages ──────────────────────────────────
// The two stages fail for completely different reasons and must be recovered
// from differently:
//
//   - **import** (`() => import('./actions/foo')`) fails for TRANSIENT,
//     environmental reasons: a network blip, a proxy hiccup, or — the common one
//     in production — ANY deploy while a tab is open, which invalidates the
//     hashed chunk URLs the already-loaded page still holds. Retrying is
//     correct, and memoizing the rejection is actively harmful: it bricks that
//     action for the rest of the session even after the network recovers.
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
): LazyActionDispatcher {
  let implPromise: Promise<(...args: any[]) => any> | null = null
  let buildFailures = 0

  /** Import with bounded retry + linear backoff. Rejects with the LAST error. */
  const importWithRetry = async (): Promise<M> => {
    let lastError: unknown
    for (let attempt = 0; attempt <= MAX_IMPORT_RETRIES; attempt++) {
      if (attempt > 0) await delay(IMPORT_RETRY_BACKOFF_MS * attempt)
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
        // re-downloads the chunk. One blip must never brick the action for the
        // session.
        implPromise = null
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

  const dispatch = ((...args: any[]) =>
    resolveImpl().then(impl => impl(...args))) as LazyActionDispatcher

  dispatch.preload = () => resolveImpl().then(() => undefined)
  return dispatch
}
