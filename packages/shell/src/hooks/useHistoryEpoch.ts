/**
 * useHistoryEpoch — a counter that increments on every history navigation.
 *
 * ## Why not `useLocation()`
 *
 * The consumer is `AppShell`'s per-module error boundary, which sits ABOVE the
 * router: the router is itself one of the module components AppShell renders, so
 * the react-router context does not exist at that level. A boundary that wants to
 * know "did the user navigate?" therefore cannot use `useLocation`.
 *
 * ## Why an epoch at all
 *
 * An error boundary LATCHES: once it catches, it renders its fallback until
 * something resets it. Without a reset signal a single crash in a module was
 * permanent for the life of the tab — which is exactly what was observed in
 * production (an explorer log shows the URL advancing across four subsequent
 * steps while the surface stayed dead). Navigating away is the natural "give it
 * another go" signal: the user has moved on, and whatever state produced the
 * throw is likely gone.
 *
 * This resets the BOUNDARY only. It never reloads the page and never remounts a
 * healthy module — see `ModuleErrorFallback` for why automatic reloads are
 * deliberately not done here.
 *
 * ## Implementation
 *
 * `popstate` covers back/forward, but SPA navigation goes through
 * `history.pushState` / `replaceState`, which fire no event. So the module patches
 * both ONCE, call-through-then-notify (never swallowing or altering the call), and
 * exposes the result through `useSyncExternalStore`. Patching is install-once per
 * MODULE INSTANCE (a single `installed` flag), so repeated mounts cannot stack
 * wrappers; two copies of `@ziee/shell` in one bundle would each install one,
 * which is harmless (each call-through still forwards) but would double-notify.
 * The patch is deliberately never uninstalled — the shell lives for the life of
 * the document, and restoring it would race any other holder of the wrapper.
 */
import { useSyncExternalStore } from 'react'

let epoch = 0
const listeners = new Set<() => void>()
let installed = false

function bump() {
  epoch += 1
  for (const l of listeners) l()
}

function install() {
  if (installed || typeof window === 'undefined') return
  installed = true

  window.addEventListener('popstate', bump)
  window.addEventListener('hashchange', bump)

  for (const method of ['pushState', 'replaceState'] as const) {
    const original = history[method]
    // Call through FIRST so the navigation is already applied when subscribers
    // read `location`, then notify. Never swallow, never alter arguments.
    history[method] = function patched(
      this: History,
      ...args: Parameters<History['pushState']>
    ) {
      const result = original.apply(this, args)
      bump()
      return result
    }
  }
}

const subscribe = (onStoreChange: () => void) => {
  install()
  listeners.add(onStoreChange)
  return () => {
    listeners.delete(onStoreChange)
  }
}

const getSnapshot = () => epoch

/**
 * Returns a number that changes on every history navigation (push / replace /
 * pop / hashchange). Stable across re-renders that are not navigations.
 */
export function useHistoryEpoch(): number {
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
}
