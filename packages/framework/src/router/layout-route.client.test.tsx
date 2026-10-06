// @vitest-environment jsdom
//
// X4 / #506 — the RENDER half of the layout-key fix, through the REAL
// `RouterComponent`.
//
// The node:test suite (`layout-route.test.ts`) proves `layoutRouteKey` returns
// DISTINCT strings per layout def and that a lazy layout suspends against its
// OWN boundary. This suite proves what that is FOR: two different lazy shells,
// both registered as route groups with their own routes, must each mount
// around ITS OWN routes when the app boots at one of them.
//
// It renders through `RouterComponent` (not a hand-built replica) so the whole
// path the app exercises is covered: `routes` store → `renderRoutesForLayoutGroup`
// → `key={layoutRouteKey(def)}` → `LayoutRouteElement` → `RenderComponentLike` →
// `Suspense`. The chunks are resolved BEFORE the first render on purpose: with
// code-split shells a returning visitor's browser already holds both, and that
// is the shape in which the reconciler sees two group `<Route>`s as siblings —
// the shape the pre-fix `component.name` key put at risk (an exotic
// `React.lazy` object has no `.name`, so every lazy group keyed as the literal
// 'layout'). `renderToStaticMarkup` cannot observe sibling-fiber behaviour at
// all (Fizz does not reuse fibers the way the client reconciler does), which is
// why this suite mounts under jsdom like the kit's own interaction tests.
import { act, lazy } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, describe, expect, it } from 'vitest'
import { RouterComponent, useRoutesStore } from './index.ts'
import type { LayoutDefinition } from './types.ts'

const PageA = () => <span>PAGE-A</span>
const PageB = () => <span>PAGE-B</span>

const siteLoader = () => import('./__test-fixtures__/SiteLayoutFixture.tsx')
const readerLoader = () => import('./__test-fixtures__/ReaderLayoutFixture.tsx')

let host: HTMLDivElement | null = null
let root: Root | null = null

afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  host = null
  root = null
  useRoutesStore.getState().resetRoutes()
})

/**
 * Boot the REAL router at `url` with two lazy layout groups registered:
 * `/a` under the SITE shell, `/b` under the READER shell.
 */
async function bootRouter(url: string): Promise<string> {
  await Promise.all([siteLoader(), readerLoader()])
  await new Promise(resolve => setTimeout(resolve, 0))
  const site: LayoutDefinition = { component: lazy(siteLoader) }
  const reader: LayoutDefinition = { component: lazy(readerLoader) }
  useRoutesStore.getState().resetRoutes()
  useRoutesStore.getState().addRoutes([
    { path: '/a', element: <PageA />, requiresAuth: false, layout: site },
  ])
  useRoutesStore.getState().addRoutes([
    { path: '/b', element: <PageB />, requiresAuth: false, layout: reader },
  ])
  window.history.pushState({}, '', url)
  host = document.createElement('div')
  document.body.appendChild(host)
  root = createRoot(host)
  await act(async () => {
    root!.render(<RouterComponent />)
  })
  const html = host.innerHTML
  act(() => root!.unmount())
  root = null
  useRoutesStore.getState().resetRoutes()
  return html
}

describe('X4 layout keying (client)', () => {
  it('two LAZY layouts each render THEIR OWN route, with no key collision', async () => {
    // /b belongs to the READER group only: its shell must wrap its own page.
    const atB = await bootRouter('/b')
    expect(atB).toMatch(/READER-SHELL\[/)
    expect(atB).not.toMatch(/SITE-SHELL/)
    expect(atB).toContain('PAGE-B')

    // /a belongs to the SITE group only — the mirror-image control, so a blurry
    // "one shell wins" pass cannot sneak through.
    const atA = await bootRouter('/a')
    expect(atA).toMatch(/SITE-SHELL\[/)
    expect(atA).not.toMatch(/READER-SHELL/)
    expect(atA).toContain('PAGE-A')
  })
})
