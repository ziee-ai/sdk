/**
 * Network-idle tracker — lets background work (the store-kit action-chunk
 * prefetch) wait until the page's CRITICAL data loads have finished before
 * competing for the browser's handful of connections.
 *
 * `api-client/core.ts` brackets every NON-SSE request with
 * `netRequestStart()`/`netRequestEnd()` (SSE streams are long-lived and NOT
 * counted). `onNetworkIdle(cb)` fires `cb` once the page has loaded AND no
 * request has been in flight for a quiet window (reset on every new request,
 * like Playwright's `networkidle`) — or after a hard cap, so a slow / polling
 * backend never starves the callback forever.
 */

let inFlight = 0
let idleTimer: ReturnType<typeof setTimeout> | null = null
let waiters: Array<() => void> = []

/** Network counts as idle only after this quiet window — long enough that a
 *  burst of parallel cold-load calls (which fire a few hundred ms apart) all
 *  settle first. Mirrors Playwright's 500ms networkidle. */
const IDLE_QUIET_MS = 500
/** Hard cap: run waiters anyway after this, so a continuously-polling or stuck
 *  backend never permanently starves the prefetch. */
const MAX_WAIT_MS = 8000

function fireIdle(): void {
  idleTimer = null
  const cbs = waiters
  waiters = []
  for (const cb of cbs) cb()
}

function armIdleTimer(): void {
  if (idleTimer) clearTimeout(idleTimer)
  idleTimer = setTimeout(fireIdle, IDLE_QUIET_MS)
}

export function netRequestStart(): void {
  inFlight++
  // A new request means we're no longer idle — cancel any pending idle fire.
  if (idleTimer) {
    clearTimeout(idleTimer)
    idleTimer = null
  }
}

export function netRequestEnd(): void {
  if (inFlight > 0) inFlight--
  // Back to idle (for now) — (re)start the quiet window; a subsequent
  // netRequestStart resets it, so we only fire IDLE_QUIET_MS after the LAST call.
  if (inFlight === 0 && waiters.length) armIdleTimer()
}

/** Run `cb` once the page has loaded and the network has been quiet for
 *  `IDLE_QUIET_MS` (or after `MAX_WAIT_MS`). Fires synchronously off-browser. */
export function onNetworkIdle(cb: () => void): void {
  if (typeof window === 'undefined' || typeof document === 'undefined') {
    cb()
    return
  }
  let fired = false
  const once = () => {
    if (fired) return
    fired = true
    cb()
  }
  // Hard cap — independent of load/idle, so nothing starves the callback.
  setTimeout(once, MAX_WAIT_MS)

  const begin = () => {
    waiters.push(once)
    // If the network is already quiet at this point, start the quiet window now;
    // otherwise netRequestEnd() will arm it when the in-flight burst drains.
    if (inFlight === 0) armIdleTimer()
  }
  // Anchor past the initial page load so a pre-load lull can't be mistaken for
  // "loads done" (the critical API calls fire around / after load on an SPA).
  if (document.readyState === 'complete') begin()
  else window.addEventListener('load', begin, { once: true })
}
