// ============================================================================
// Lazy-action dispatch — extracted from store-kit so it can be reasoned about
// (and unit-tested) on its own, with no zustand/EventBus dependency graph.
//
// A lazy action lives in its own chunk: calling it must (1) download the chunk,
// (2) build the impl from `(set, get)`, then (3) invoke it. The naive form
//
//     const dispatch = (...a) => resolveImpl().then(impl => impl(...a))
//
// has a correctness hole: the action BODY — and therefore the action's OWN
// in-flight guard (`if (state.loading) return`) — cannot run until the chunk
// resolves. Two callers in the same tick (two mounted consumers, a store
// re-init, a remount) therefore BOTH get past every guard and issue the same
// request twice. Every list store in the app carries such a guard, and during
// its own chunk load none of them work.
//
// `createLazyDispatcher` closes exactly that window and nothing else: calls made
// while the impl is still loading share one invocation (keyed by arguments);
// once it has resolved, dispatch is byte-identical to the naive form, so
// repeated calls — including repeated mutations — still repeat.
// ============================================================================

/** Loads one action module and builds its impl. */
export type ImplLoader = () => Promise<(...args: any[]) => any>

export interface LazyDispatcher {
  (...args: any[]): Promise<any>
  /** Warm the chunk + build the impl without invoking it. */
  preload: () => Promise<void>
}

export function createLazyDispatcher(loadImpl: ImplLoader): LazyDispatcher {
  let implPromise: Promise<(...args: any[]) => any> | null = null
  // Flips once the impl exists — i.e. once the action's own guard is reachable.
  let implReady = false
  const resolveImpl = () =>
    (implPromise ??= loadImpl().then(impl => {
      implReady = true
      return impl
    }))

  // Calls made during the chunk-load window, keyed by their arguments. Cleared
  // as each settles; empty (and never consulted) in steady state.
  const coldCalls = new Map<string, Promise<any>>()

  const dispatch = ((...args: any[]) => {
    if (implReady) return resolveImpl().then(impl => impl(...args))
    let argKey: string
    try {
      argKey = JSON.stringify(args)
    } catch {
      // Non-serializable args (a callback, a cyclic object) — never share, since
      // we cannot prove two such calls are equivalent.
      return resolveImpl().then(impl => impl(...args))
    }
    const pending = coldCalls.get(argKey)
    if (pending) return pending
    const run = resolveImpl()
      .then(impl => impl(...args))
      .finally(() => {
        coldCalls.delete(argKey)
      })
    coldCalls.set(argKey, run)
    return run
  }) as LazyDispatcher

  dispatch.preload = () => resolveImpl().then(() => undefined)
  return dispatch
}
