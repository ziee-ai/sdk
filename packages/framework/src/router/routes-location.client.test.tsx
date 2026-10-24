// @vitest-environment jsdom
//
// PROV-sdk-1's OWN test, in the SDK (the cross-epic acceptance rule — the SDK
// change's own test ships in the SDK; the consumer's test asserts the seam's
// USE). THREE pins through the REAL `RouterComponent` (never a hand-built
// replica):
//   1. the NULL default: with no `routesLocation` the router paints its OWN
//      location (today's render — byte-identical in every other app);
//   2. a HELD location renders the OLD route while the URL moved — the render
//      follows the override, and the held route's `useLocation`/`useParams`
//      read the HELD values (RR's `<Routes location>` re-provision);
//   3. the SUBSCRIPTION re-renders on release: clearing the held value fires
//      the change listener and the committed URL's route paints.
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { useLocation, useNavigate, useParams } from 'react-router-dom'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { useModuleSystemStore } from '../module-system/store'
import {
  RouterComponent,
  setRouterConfig,
  useRoutesStore,
  type RouterLocation,
} from './index.ts'

// React's act() flush (the jsdom mount pattern the layout suite uses).
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const PageB = () => <span>PAGE-B</span>

/** The held route's context witness (the re-provision's proof). */
function LocationProbe() {
  const location = useLocation()
  const params = useParams()
  return (
    <span>
      LOC:{location.pathname}|KEY:{location.key}|PARAMS:{String(params.slug ?? '')}
    </span>
  )
}

/** The navigation harness mounted INSIDE the router (a `routerEffects` entry —
 *  the only way to make the router commit a real navigation from a test; a raw
 *  `window.history.pushState` is invisible to react-router's own history). */
let navigateFn: ((to: string) => void) | null = null
function NavHarness() {
  const navigate = useNavigate()
  navigateFn = navigate
  return null
}

let host: HTMLDivElement | null = null
let root: Root | null = null

/** The app-side holder's two facts (the seam's getter + subscription face). */
let held: RouterLocation | null = null
const listeners = new Set<() => void>()
function subscribeHeld(onChange: () => void): () => void {
  listeners.add(onChange)
  return () => {
    listeners.delete(onChange)
  }
}
function setHeld(value: RouterLocation | null): void {
  held = value
  for (const listener of [...listeners]) listener()
}

const ROUTE_A: RouterLocation = { pathname: '/a', search: '', hash: '', key: 'ka', state: undefined }

function resetSeam(): void {
  held = null
  listeners.clear()
  navigateFn = null
  setRouterConfig({
    // The null default — a test must not leak the wiring into the next case.
    routesLocation: undefined,
    routesLocationSubscribe: undefined,
  })
}

beforeEach(() => {
  resetSeam()
  useRoutesStore.getState().resetRoutes()
  useRoutesStore.getState().addRoutes([
    { path: '/a', element: <LocationProbe />, requiresAuth: false },
    { path: '/b', element: <PageB />, requiresAuth: false },
  ])
  useModuleSystemStore.getState().slots.set('routerEffects', [
    { id: 'route-location-test-nav', component: NavHarness },
  ])
  host = document.createElement('div')
  document.body.appendChild(host)
})

afterEach(() => {
  act(() => root?.unmount())
  root = null
  host?.remove()
  host = null
  useRoutesStore.getState().resetRoutes()
  useModuleSystemStore.getState().slots.delete('routerEffects')
  resetSeam()
})

/** Boot the real RouterComponent at `url` (the URL the app boots at). */
function bootAt(url: string): void {
  window.history.pushState({}, '', url)
  root = createRoot(host!)
  act(() => {
    root!.render(<RouterComponent />)
  })
}

describe('PROV-sdk-1 — the router location seam (the SDK reads the app-supplied location)', () => {
  it('null default: with no routesLocation the router paints its OWN location (today’s render)', () => {
    bootAt('/b')
    expect(host!.innerHTML).toContain('PAGE-B')
    expect(host!.innerHTML).not.toContain('LOC:')
  })

  it('a held location renders the OLD route while the URL moved (context re-provision)', () => {
    setRouterConfig({
      routesLocation: () => held,
      routesLocationSubscribe: subscribeHeld,
    })
    bootAt('/a')
    act(() => setHeld(ROUTE_A))

    // The router COMMITS /b while /a is held: the render stays at the OLD
    // route and the held route's useLocation/useParams read the HELD values.
    act(() => navigateFn!('/b'))
    expect(host!.innerHTML).toContain('LOC:/a|KEY:ka|PARAMS:')
    expect(host!.innerHTML).not.toContain('PAGE-B')
  })

  it('the subscription re-renders on release: the committed URL’s route paints', () => {
    setRouterConfig({
      routesLocation: () => held,
      routesLocationSubscribe: subscribeHeld,
    })
    bootAt('/a')
    act(() => setHeld(ROUTE_A))
    act(() => navigateFn!('/b'))
    expect(host!.innerHTML).toContain('LOC:/a|KEY:ka')

    // Release: the holder's change listener fires the router's re-render and
    // the committed URL's route paints.
    act(() => setHeld(null))
    expect(host!.innerHTML).toContain('PAGE-B')
    expect(host!.innerHTML).not.toContain('LOC:')
  })

  it('without the subscription the getter is still read at mount, but a holder change does NOT repaint', () => {
    // The getter alone is read at the mount (the painter's first read), but a
    // holder change with NO navigation leaves the paint stale —
    // useSyncExternalStore's subscribe is the re-render trigger the recorded
    // requirement names. The app wires it (installRouterLocationSeam writes
    // both fields).
    setRouterConfig({ routesLocation: () => held })
    act(() => setHeld(ROUTE_A))
    bootAt('/b')
    expect(host!.innerHTML).toContain('LOC:/a|KEY:ka')
    expect(host!.innerHTML).not.toContain('PAGE-B')
    // The release with no subscription: no re-render, the stale paint stays.
    act(() => setHeld(null))
    expect(host!.innerHTML).toContain('LOC:/a|KEY:ka')
  })
})
