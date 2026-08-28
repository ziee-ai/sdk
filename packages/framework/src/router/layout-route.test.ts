import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Suspense, createElement as h, lazy } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import {
  LayoutRouteElement,
  __resetLayoutRouteKeys,
  layoutRouteKey,
} from './layout-route.tsx'
import type { LayoutDefinition } from './types.ts'

// X4 (reported by COMIZY, which code-splits five route-level shells).
//
// `RouterComponent` keyed a layout group's parent <Route> as
//
//     key={layoutDef.component.name || 'layout'}
//
// and mounted the layout as a bare `<LayoutComponent><Outlet/></LayoutComponent>`
// with no Suspense. Both halves break the moment a layout is lazy — which is the
// only reason to split it out in the first place:
//
//   1. `React.lazy(...)` is an exotic OBJECT, so `.name` is undefined and EVERY
//      lazy layout keys as the literal 'layout'. Two adjacent layout groups then
//      share a key, and React reconciles the second INTO the first: routes
//      declared under the reader shell render inside the site shell.
//   2. With no boundary, a lazy layout's suspension escapes to the app-level
//      boundary and blanks the whole app on every layout switch.

const lazySite = lazy(() => import('./__test-fixtures__/SiteLayoutFixture.tsx'))
const lazyReader = lazy(() => import('./__test-fixtures__/ReaderLayoutFixture.tsx'))

/** What the router used to derive the key from. */
const nameKey = (def: LayoutDefinition) =>
  (def.component as { name?: string }).name || 'layout'

test('X4: two LAZY layouts get DISTINCT route keys (they used to collide)', () => {
  __resetLayoutRouteKeys()
  const site: LayoutDefinition = { component: lazySite }
  const reader: LayoutDefinition = { component: lazyReader }

  // The defect, stated as the property that used to hold.
  assert.equal(nameKey(site), 'layout')
  assert.equal(nameKey(reader), 'layout')
  assert.equal(nameKey(site), nameKey(reader), 'precondition: name-keying collided')

  assert.notEqual(
    layoutRouteKey(site),
    layoutRouteKey(reader),
    'two distinct layout defs must never share a React key — React reconciles ' +
      'same-keyed adjacent Routes into one, so the second shell silently vanishes',
  )
})

test('X4: a bare-loader layout also gets a distinct key', () => {
  // `{ component: () => import('./X') }` is a NAMED arrow (`component`), so
  // name-keying gave every one of them the same key 'component'.
  __resetLayoutRouteKeys()
  const a: LayoutDefinition = { component: () => import('./__test-fixtures__/SiteLayoutFixture.tsx') }
  const b: LayoutDefinition = { component: () => import('./__test-fixtures__/ReaderLayoutFixture.tsx') }
  assert.equal(nameKey(a), nameKey(b), 'precondition: both arrows are named `component`')
  assert.notEqual(layoutRouteKey(a), layoutRouteKey(b))
})

test('layoutRouteKey is STABLE for one def and honours an explicit id', () => {
  __resetLayoutRouteKeys()
  const def: LayoutDefinition = { component: lazySite }
  assert.equal(layoutRouteKey(def), layoutRouteKey(def))
  assert.equal(layoutRouteKey({ id: 'site', component: lazySite }), 'layout:site')
  // Two defs sharing ONE component object are still two layouts.
  assert.notEqual(
    layoutRouteKey({ component: lazySite }),
    layoutRouteKey({ component: lazySite }),
  )
})

// ── the Suspense half ───────────────────────────────────────────────────────

const APP_FALLBACK = 'APP-BOUNDARY-FALLBACK'
const LAYOUT_FALLBACK = 'LAYOUT-FALLBACK'

const renderLayout = (def: LayoutDefinition) =>
  renderToStaticMarkup(
    h(
      Suspense,
      { fallback: APP_FALLBACK },
      h(LayoutRouteElement, {
        layoutDef: def,
        fallback: LAYOUT_FALLBACK,
        children: h('span', null, 'PAGE'),
      }),
    ),
  )

test('X4: a lazy layout suspends against the ROUTER\'s boundary, not the app\'s', () => {
  const markup = renderLayout({ component: lazySite })
  assert.equal(
    markup,
    LAYOUT_FALLBACK,
    'the suspension escaped to the app-level boundary — which is what blanks the ' +
      'ENTIRE app for the duration of a layout switch instead of just the shell',
  )
})

test('X4: the lazy layout renders its shell around the route content once loaded', async () => {
  const loader = () => import('./__test-fixtures__/SiteLayoutFixture.tsx')
  const def: LayoutDefinition = { component: loader }
  renderLayout(def) // start the chunk
  await loader()
  await new Promise(r => setTimeout(r, 0))
  const markup = renderLayout(def)
  assert.match(markup, /SITE-SHELL\[/, 'the shell never mounted')
  assert.match(markup, /PAGE/, 'route content was not passed through as `children`')
})

test('an EAGER layout renders directly, with no boundary of its own', () => {
  function EagerLayout({ children }: { children: unknown }) {
    return h('div', null, 'EAGER[', children as never, ']')
  }
  const markup = renderLayout({ component: EagerLayout })
  assert.match(markup, /EAGER\[/)
  assert.match(markup, /PAGE/)
  assert.ok(!markup.includes(LAYOUT_FALLBACK) && !markup.includes(APP_FALLBACK))
})
