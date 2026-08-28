import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Component, Suspense, createElement as h, forwardRef, lazy, memo } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { LazyRouteRenderer } from './LazyRouteRenderer.tsx'
import {
  __resetLazyComponentWarnings,
  classifyComponentLike,
  markLazyLoader,
} from '../lazy-component.ts'

// GAP-7 (reported by COMIZY while wiring its C1 slot mounts).
//
// A module declares its lazy screens the documented way:
//
//     { component: () => import('./LazyPage') }
//
// The arrow is an object-property initializer, so JS infers `.name === 'component'`.
// `LazyRouteRenderer` decided "is this a dynamic-import loader?" by asking whether the
// function was ANONYMOUS — so this entry was classified as a plain React component,
// invoked, and returned a Promise. React then suspends on an UNCACHED promise (a fresh
// one per call), so the nearest boundary — the app's, not the renderer's — never
// resolves: the region renders the app fallback forever, with no thrown error and no
// console output. A silently empty region.
//
// Both assertions below are about that, and both are rendered through an OUTER Suspense
// boundary, because a real app always has one (AppShell / AppLayout) and it is the outer
// boundary that swallowed the failure.

/** The app-level boundary that hid the bug. */
const APP_FALLBACK = 'APP-BOUNDARY-FALLBACK'
/** The boundary `LazyRouteRenderer` is supposed to own for a lazy entry. */
const ROUTE_FALLBACK = 'ROUTE-FALLBACK'

const entry = { component: () => import('./__test-fixtures__/LazyPage.tsx') }

const renderEntry = () =>
  renderToStaticMarkup(
    h(
      Suspense,
      { fallback: APP_FALLBACK },
      h(LazyRouteRenderer, {
        component: entry.component as never,
        fallback: ROUTE_FALLBACK,
      }),
    ),
  )

test('GAP-7: a `{ component: () => import(...) }` entry is bounded by the RENDERER, not punted to the app boundary', () => {
  const markup = renderEntry()
  assert.equal(
    markup,
    ROUTE_FALLBACK,
    'the renderer did not recognise the dynamic-import loader: the suspension escaped to the ' +
      'app-level boundary, which is what makes the region render empty with no error',
  )
})

test('GAP-7: the loaded module actually renders once its chunk resolves', async () => {
  renderEntry() // first pass starts the loader
  await entry.component() // let the chunk settle
  await new Promise(r => setTimeout(r, 0))
  const markup = renderEntry()
  assert.match(
    markup,
    /LAZY PAGE CONTENT/,
    'the resolved module never reached the DOM — the region stays empty for the life of the page',
  )
})

// ── The shapes that must KEEP working (regression guard on the fix) ─────────

test('an already-built element, a props-taking component, and a class component still render directly', () => {
  const El = h('p', null, 'ELEMENT')
  assert.match(renderToStaticMarkup(h(LazyRouteRenderer, { component: El as never })), /ELEMENT/)

  function Page({ label }: { label?: string }) {
    return h('p', null, label ?? 'COMPONENT')
  }
  assert.match(
    renderToStaticMarkup(h(LazyRouteRenderer, { component: Page as never })),
    /COMPONENT/,
  )

  class ClassPage extends Component {
    render() {
      return h('p', null, 'CLASS')
    }
  }
  assert.match(
    renderToStaticMarkup(h(LazyRouteRenderer, { component: ClassPage as never })),
    /CLASS/,
  )
})

test('an anonymous loader (the shape the old name heuristic DID handle) still gets its own boundary', () => {
  // An array-element initializer, so JS infers no name for the arrow.
  const anonymous = [() => import('./__test-fixtures__/LazyPage.tsx')][0]
  assert.equal(anonymous.name, '', 'precondition: this loader is anonymous')
  assert.equal(
    renderToStaticMarkup(
      h(
        Suspense,
        { fallback: APP_FALLBACK },
        h(LazyRouteRenderer, { component: anonymous as never, fallback: ROUTE_FALLBACK }),
      ),
    ),
    ROUTE_FALLBACK,
  )
})

test('a React.lazy() exotic is recognised structurally, by $$typeof', () => {
  const LazyExotic = lazy(() => import('./__test-fixtures__/LazyPage.tsx'))
  assert.equal(classifyComponentLike(LazyExotic).kind, 'react-lazy')
  assert.equal(
    renderToStaticMarkup(
      h(
        Suspense,
        { fallback: APP_FALLBACK },
        h(LazyRouteRenderer, { component: LazyExotic as never, fallback: ROUTE_FALLBACK }),
      ),
    ),
    ROUTE_FALLBACK,
  )
})

// ── Classification is by shape/marker, not by name ──────────────────────────

test('classification: every loader spelling is recognised WITHOUT relying on the function name', () => {
  const named = { component: () => import('./__test-fixtures__/LazyPage.tsx') }
  assert.equal(named.component.name, 'component', 'precondition: the arrow is NOT anonymous')
  assert.equal(classifyComponentLike(named.component).kind, 'loader')
  assert.equal(classifyComponentLike(named.component).reason, 'dynamic import() in source')

  // A different field name — the exact next-spelling case a widened name
  // pattern would still have missed.
  const differentKey = { element: () => import('./__test-fixtures__/LazyPage.tsx') }
  assert.equal(classifyComponentLike(differentKey.element).kind, 'loader')

  // A bundler-rewritten loader: the specifier and the surrounding identifiers
  // are renamed, `import(` is not.
  const preload = (fn: () => Promise<unknown>) => fn()
  const bundled = { component: () => preload(() => import('./__test-fixtures__/LazyPage.tsx')) }
  assert.equal(classifyComponentLike(bundled.component as never).kind, 'loader')

  // The explicit marker, for a loader whose source shows no `import(` at all.
  const opaque = { component: markLazyLoader(() => Promise.resolve({ default: () => null })) }
  assert.equal(classifyComponentLike(opaque.component).kind, 'loader')
  assert.equal(classifyComponentLike(opaque.component).reason, 'markLazyLoader()')
})

test('classification: a props-taking function is never mistaken for a loader', () => {
  const withProps = { component: (props: { a: number }) => h('p', null, props.a) }
  assert.equal(classifyComponentLike(withProps.component).kind, 'component')
  assert.equal(classifyComponentLike(withProps.component).ambiguous, false)
})

// ── The residual ambiguity is LOUD, not silent ─────────────────────────────

test('GAP-7: a loader that shape detection cannot see is reported by name instead of rendering empty forever', () => {
  __resetLazyComponentWarnings()
  const errors: string[] = []
  const realError = console.error
  console.error = (...args: unknown[]) => {
    errors.push(args.map(String).join(' '))
  }
  try {
    // No `import(` in its source and a name — indistinguishable from a
    // component by shape. Rendering it used to suspend forever, silently.
    const loadPage = () => Promise.resolve({ default: () => h('p', null, 'X') })
    const markup = renderToStaticMarkup(
      h(
        Suspense,
        { fallback: APP_FALLBACK },
        h(LazyRouteRenderer, {
          component: loadPage as never,
          fallback: ROUTE_FALLBACK,
          debugId: '/series/:id',
        }),
      ),
    )
    // It renders nothing — but it says so, and it does NOT leave the app
    // boundary suspended.
    assert.equal(markup, '')
    assert.equal(errors.length, 1, `expected exactly one diagnostic, got: ${JSON.stringify(errors)}`)
    assert.match(errors[0], /\/series\/:id/, 'the diagnostic must name the offending entry')
    assert.match(errors[0], /loadPage/)
    assert.match(errors[0], /markLazyLoader/, 'the diagnostic must say how to fix it')
  } finally {
    console.error = realError
  }
})

test('GAP-7: an unrenderable value names the slot instead of rendering nothing quietly', () => {
  __resetLazyComponentWarnings()
  const errors: string[] = []
  const realError = console.error
  console.error = (...args: unknown[]) => {
    errors.push(args.map(String).join(' '))
  }
  try {
    const markup = renderToStaticMarkup(
      h(LazyRouteRenderer, { component: undefined as never, debugId: 'homeSections' }),
    )
    assert.equal(markup, '')
    assert.equal(errors.length, 1)
    assert.match(errors[0], /homeSections/)
  } finally {
    console.error = realError
  }
})

// ── React EXOTIC component types (review follow-up to the fix above) ────────
//
// `React.memo(...)` / `React.forwardRef(...)` are OBJECTS carrying a `$$typeof`
// symbol, not functions. The first cut of the shape classifier tested for
// `react.lazy` and then fell through to `typeof value !== 'function' →
// 'invalid'`, so a memo'd or ref-forwarding slot entry rendered NOTHING and
// logged an "not renderable" diagnostic — the same silent-empty-region class of
// failure this module exists to end, reintroduced for exotics. The pre-fix
// renderers got these right by accident (they only special-cased functions).

test('a React.memo component is a renderable type, not an "invalid" value', () => {
  const Memo = memo(function Inner() {
    return h('p', null, 'MEMO')
  })
  const c = classifyComponentLike(Memo)
  assert.equal(c.kind, 'component', `memo exotic classified ${c.kind} (${c.reason})`)
  assert.match(
    renderToStaticMarkup(h(LazyRouteRenderer, { component: Memo as never })),
    /MEMO/,
  )
})

test('a React.forwardRef component is a renderable type, not an "invalid" value', () => {
  const Fwd = forwardRef(function Inner() {
    return h('p', null, 'FWD')
  })
  const c = classifyComponentLike(Fwd)
  assert.equal(c.kind, 'component', `forwardRef exotic classified ${c.kind} (${c.reason})`)
  assert.match(
    renderToStaticMarkup(h(LazyRouteRenderer, { component: Fwd as never })),
    /FWD/,
  )
})

test('memo(lazy(...)) still suspends, so it still gets the renderer-owned boundary', () => {
  const MemoLazy = memo(lazy(() => import('./__test-fixtures__/LazyPage.tsx')))
  assert.equal(classifyComponentLike(MemoLazy).kind, 'react-lazy')
  assert.equal(
    renderToStaticMarkup(
      h(
        Suspense,
        { fallback: APP_FALLBACK },
        h(LazyRouteRenderer, { component: MemoLazy as never, fallback: ROUTE_FALLBACK }),
      ),
    ),
    ROUTE_FALLBACK,
  )
})

// ── The ambiguous-component tripwire must not merge two components' fibers ──

test('two different ambiguous components never share one fiber (identity key)', () => {
  // Both are named 0-arg functions with no `import(` in their source, so both
  // take the `ambiguous` path through the shared tripwire component. Rendering
  // them under one type WITHOUT a key would reconcile the second into the
  // first's fiber and carry its hook state across.
  function First() {
    return h('p', null, 'FIRST')
  }
  function Second() {
    return h('p', null, 'SECOND')
  }
  assert.equal(classifyComponentLike(First).ambiguous, true, 'precondition: ambiguous')
  assert.equal(classifyComponentLike(Second).ambiguous, true, 'precondition: ambiguous')

  const markup = renderToStaticMarkup(
    h(
      'div',
      null,
      h(LazyRouteRenderer, { component: First as never }),
      h(LazyRouteRenderer, { component: Second as never }),
    ),
  )
  assert.match(markup, /FIRST/)
  assert.match(markup, /SECOND/)
})
