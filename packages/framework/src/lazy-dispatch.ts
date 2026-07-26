// ============================================================================
// Lazy-action dispatch — extracted from store-kit so it can be reasoned about
// (and unit-tested) on its own, with no zustand/EventBus dependency graph.
//
// A lazy action lives in its own chunk: calling it must (1) download the chunk,
// (2) build the impl from `(set, get)`, then (3) invoke it. The chunk + impl are
// memoized so the download happens once; `preload()` warms them without
// invoking the action.
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

/** Loads one action module and builds its impl. */
export type ImplLoader = () => Promise<(...args: any[]) => any>

/** The callable a lazy action becomes on a store. Named for the ACTION it wraps
 *  (store-kit exports a differently-shaped, fully-typed `LazyDispatcher<L>` for
 *  the same concept at the type level — these must not share a name). */
export interface LazyActionDispatcher {
  (...args: any[]): Promise<any>
  /** Warm the chunk + build the impl without invoking it. */
  preload: () => Promise<void>
}

/**
 * How many times a failed resolve is retried before the rejection is memoized.
 *
 * Clearing the memo on failure is what stops a transient chunk 404 / network blip
 * bricking an action for the session. But `loadImpl` also runs the store's action
 * FACTORY, and a throw from there is a deterministic authoring bug — retrying it
 * forever would turn one bug into an unbounded loop for a component that
 * dispatches from a render or an effect. One retry covers the transient case; the
 * second failure is treated as deterministic and memoized, so the action fails
 * fast and loudly from then on.
 */
const MAX_RESOLVE_RETRIES = 1

export function createLazyDispatcher(loadImpl: ImplLoader): LazyActionDispatcher {
  let implPromise: Promise<(...args: any[]) => any> | null = null
  let failures = 0
  const resolveImpl = () => {
    if (implPromise) return implPromise
    implPromise = loadImpl().catch(err => {
      failures++
      if (failures <= MAX_RESOLVE_RETRIES) implPromise = null
      throw err
    })
    return implPromise
  }

  const dispatch = ((...args: any[]) =>
    resolveImpl().then(impl => impl(...args))) as LazyActionDispatcher

  dispatch.preload = () => resolveImpl().then(() => undefined)
  return dispatch
}
