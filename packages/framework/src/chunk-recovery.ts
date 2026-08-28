// ============================================================================
// Dynamic-import (code-split chunk) failure recovery.
//
// The app code-splits aggressively — one chunk per store action — so a single
// bad moment can fail many chunks at once. Two things then happen that the app
// used to handle nowhere:
//
//  1. Vite's `__vitePreload` raises a `vite:preloadError` event when a chunk (or
//     one of its preloaded deps) fails to load.
//  2. The most common cause is not a network fault at all: it is ANY DEPLOY
//     WHILE A TAB IS OPEN. The loaded page holds hashed chunk URLs that no
//     longer exist on the server, so every not-yet-loaded lazy surface 404s for
//     the rest of that page's life.
//
// This module owns exactly ONE responsibility: record (2) — set a process-wide
// "this page is running against a build the server no longer serves" mark that
// user-facing error messages consult, so the app can tell the user WHY reloading
// will fix it. Recovery itself belongs to `lazy-dispatch.ts`, which retries the
// import and, if it still fails, surfaces an actionable error to the caller.
//
// ── It must NOT call preventDefault() ───────────────────────────────────────
// Vite's helper is:
//
//     function handlePreloadError(err) {
//       const e = new Event('vite:preloadError', { cancelable: true })
//       e.payload = err; window.dispatchEvent(e)
//       if (!e.defaultPrevented) throw err          // ← the ONLY rethrow
//     }
//     return promise.then(res => { …; return baseModule().catch(handlePreloadError) })
//
// so `preventDefault()` does not merely suppress a duplicate console error — it
// makes the IMPORT PROMISE RESOLVE WITH `undefined`. The caller then reads
// `mod.default` off `undefined` and dies with a confusing
// "Cannot read properties of undefined", and the dispatcher's retry never runs
// because nothing rejected. That is the same silent-failure class this whole
// change exists to remove, so the listener is strictly an OBSERVER: it marks and
// logs, and lets Vite rethrow into the promise the dispatcher already handles.
// (Caught by the e2e — an earlier draft did preventDefault and the "self-heals
// after the blip clears" leg failed with exactly that TypeError.)
//
// It deliberately does NOT reload the page. An automatic `location.reload()`
// during a chat session destroys the unsent draft in the composer and tears down
// an in-flight assistant stream, and it can loop when the chunk is missing for a
// non-deploy reason (an offline tab, a proxy 502). The app's own root
// `AppErrorBoundary` already sets the precedent: it renders a "Reload page"
// BUTTON and lets the user choose.
// ============================================================================

/** Payload Vite attaches to `vite:preloadError`. */
interface PreloadErrorEvent extends Event {
  payload?: unknown
}

let staleBuild = false

/**
 * True while a code-split chunk load is known to be failing.
 *
 * Consumed by user-facing error messages to add "the app may have been updated —
 * reload to get the latest version", which is the actionable half of the message.
 *
 * NOT permanently sticky. An earlier draft never reset it, which contradicted the
 * dispatcher's own thesis (an import failure is TRANSIENT) and had a real cost: a
 * 300 ms wifi blip during boot latched the flag for the whole session, which in
 * turn disabled `store-kit`'s lazy-action prefetch for every store registered
 * afterwards even though the network had recovered. A SUCCESSFUL import clears it
 * (`clearStaleBuild`), so it means what it says: chunk loading is broken RIGHT
 * NOW. In a genuine deploy nearly every chunk 404s, so it stays set; after a blip
 * it clears as soon as anything loads.
 */
export function isStaleBuild(): boolean {
  return staleBuild
}

/**
 * Mark this page as running against a stale build.
 *
 * Exported so a non-Vite import failure observed elsewhere (e.g. a rejected
 * `import()` a dispatcher retried and gave up on) can record the same condition.
 */
export function markStaleBuild(): void {
  staleBuild = true
}

type ListenerTarget = Pick<Window, 'addEventListener' | 'removeEventListener'>

/**
 * Targets that already carry a listener.
 *
 * PER-TARGET, not a module-scope boolean: this module is module state shared by
 * every consumer in the process, so a single `installed` flag meant one caller
 * that never uninstalled silently disabled installation for everything after it
 * — including later tests in the same file — while handing back a no-op
 * uninstall indistinguishable from a real one. A WeakSet also lets two genuinely
 * different targets (a window and a test double) each get their listener.
 */
const installedTargets = new WeakSet<ListenerTarget>()

/**
 * Clear the stale-build mark — a code-split chunk has just loaded successfully,
 * so chunk loading is demonstrably working again.
 */
export function clearStaleBuild(): void {
  staleBuild = false
}

/** Test-only reset — BOTH pieces of module state; the install set must not leak
 *  between specs. */
export function __resetStaleBuildForTests(target?: ListenerTarget): void {
  staleBuild = false
  if (target) installedTargets.delete(target)
}

/**
 * Install the `vite:preloadError` listener. Idempotent PER TARGET: calling it
 * twice for the same target (web entry + a re-entrant bootstrap) registers
 * exactly one listener.
 *
 * @param target the event target to listen on. Defaults to `window`; injected in
 *        tests. A no-op when there is no target (SSR / a node unit context).
 * @returns an uninstall function.
 */
export function installChunkLoadRecovery(
  target: ListenerTarget | undefined = typeof window === 'undefined'
    ? undefined
    : window,
): () => void {
  if (!target || installedTargets.has(target)) return () => {}
  installedTargets.add(target)

  const onPreloadError = (event: Event) => {
    markStaleBuild()
    // Deliberately NOT preventDefault()ed — see the header. Calling it would
    // make the import promise resolve with `undefined` instead of rejecting,
    // which silently defeats both the dispatcher's retry and the caller's error
    // handling.
    console.warn(
      '[chunk-recovery] a code-split chunk failed to load; the app may have been updated since this tab opened. Reloading the page will pick up the new build.',
      (event as PreloadErrorEvent).payload,
    )
  }

  target.addEventListener('vite:preloadError', onPreloadError as EventListener)
  return () => {
    target.removeEventListener(
      'vite:preloadError',
      onPreloadError as EventListener,
    )
    installedTargets.delete(target)
  }
}
