/**
 * The background WARM-SCHEDULER seam.
 *
 * Two SDK arms warm code the page has not asked for yet:
 *
 *   - store-kit's baked-in action prefetch (`autoWarmLazyActions`) — every
 *     lazy-action chunk of every store, once that store's `init` has run;
 *   - the shell's route prefetch (`usePrefetchModules`) — every reachable
 *     route's lazy element.
 *
 * Left to themselves, each arm fires ALL of its imports in one idle callback.
 * On an app with dozens of stores and routes that is hundreds of requests in
 * the same second, right after first paint: the browser's per-process network
 * budget is exhausted (`net::ERR_INSUFFICIENT_RESOURCES`), and because a FAILED
 * module fetch is recorded in the module map for the life of the document, the
 * warm-up poisons the very chunks the next navigation needs. A weak device
 * then crashes a route the user really clicked.
 *
 * So an app may install ONE scheduler that owns the timing, the concurrency
 * cap and the order of every background warm. With none installed, both arms
 * keep their historical behaviour (idle callback, fire everything), so an app
 * that never calls `setWarmScheduler` is unaffected.
 *
 * The task is a thunk; the scheduler decides when to call it and must treat a
 * rejected or thrown task as a no-op (a failed warm-up is not an error — the
 * on-demand import retries and reports).
 */

/** Where a warm task came from — lets the scheduler rank and dedupe it. */
export interface WarmTaskMeta {
  source: 'store-actions' | 'route'
  /** A stable dedupe key: `<store>.<action>` or the route path. */
  key: string
  /** The route pattern for a `route` task (likely-next ranking). */
  path?: string
}

export type WarmTask = () => unknown

export type WarmScheduler = (task: WarmTask, meta: WarmTaskMeta) => void

let installed: WarmScheduler | null = null

/** Install (or, with `null`, remove) the app's warm scheduler. */
export function setWarmScheduler(scheduler: WarmScheduler | null): void {
  installed = scheduler
}

/** The installed scheduler, or `null` (the arms then use their legacy path). */
export function getWarmScheduler(): WarmScheduler | null {
  return installed
}
