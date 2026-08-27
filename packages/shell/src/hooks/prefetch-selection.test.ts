import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  pathMatchesCurrent,
  selectPrefetchRoutes,
  type PrefetchRoute,
} from './prefetch-selection.ts'

// A representative public-site route table: mostly public, some behind auth,
// some behind a permission. Every element is a dynamic-import loader, i.e.
// prefetchable in principle.
const loader = () => Promise.resolve({ default: () => null })
const routes: PrefetchRoute[] = [
  { path: '/', element: loader },
  { path: '/browse', element: loader },
  { path: '/series/:id', element: loader },
  { path: '/read/:id/:chapter', element: loader },
  { path: '/auth', element: loader },
  { path: '/library', element: loader, requiresAuth: true },
  { path: '/settings', element: loader, requiresAuth: true },
  { path: '/admin', element: loader, requiresAuth: true, permission: 'admin::read' },
  // Not a loader — an already-built element. Never prefetchable.
  { path: '/static', element: { $$typeof: Symbol.for('react.element') } },
]

const paths = (rs: PrefetchRoute[]) => rs.map(r => r.path).sort()

// The guest defect (reported by COMIZY). `usePrefetchModules` opened with
// `if (!isAuthed) return` — a signed-out visitor got ZERO prefetch. On a product
// whose signed-out surface is the majority traffic path, that disables the
// feature for most users, and it is silent: prefetch is invisible when it works
// and equally invisible when it does not.
test('a signed-out visitor prefetches every PUBLIC route they can actually reach', () => {
  const selected = selectPrefetchRoutes({
    routes,
    isAuthed: false,
    pathname: '/',
    hasPermission: () => {
      throw new Error('the permission check must not be consulted for a guest')
    },
  })
  assert.deepEqual(paths(selected), ['/auth', '/browse', '/read/:id/:chapter', '/series/:id'])
})

test('a signed-out visitor is never handed a chunk they cannot route to', () => {
  const selected = selectPrefetchRoutes({
    routes,
    isAuthed: false,
    pathname: '/nowhere',
    hasPermission: () => true, // even a permissive check must not open the gate
  })
  const got = paths(selected)
  assert.ok(!got.includes('/library'), 'requiresAuth route leaked to a guest')
  assert.ok(!got.includes('/settings'), 'requiresAuth route leaked to a guest')
  assert.ok(!got.includes('/admin'), 'permission-gated route leaked to a guest')
  assert.ok(!got.includes('/static'), 'a non-loader element was treated as prefetchable')
})

test('the current route is skipped — its chunk is already loading', () => {
  const selected = selectPrefetchRoutes({
    routes,
    isAuthed: false,
    pathname: '/series/42',
    hasPermission: () => false,
  })
  assert.ok(!paths(selected).includes('/series/:id'))
  assert.ok(paths(selected).includes('/browse'))
})

// The authenticated behaviour is a NEGATIVE CONTROL on the fix: it must be
// byte-for-byte what it was.
test('a signed-in visitor is unchanged: every route minus the ones their permissions deny', () => {
  const denied: unknown[] = []
  const selected = selectPrefetchRoutes({
    routes,
    isAuthed: true,
    pathname: '/library',
    hasPermission: p => {
      denied.push(p)
      return false
    },
  })
  assert.deepEqual(paths(selected), [
    '/',
    '/auth',
    '/browse',
    '/read/:id/:chapter',
    '/series/:id',
    '/settings',
  ])
  assert.deepEqual(denied, ['admin::read'], 'the permission check runs for a signed-in visitor')
})

test('a signed-in visitor with the permission gets the gated route too', () => {
  const selected = selectPrefetchRoutes({
    routes,
    isAuthed: true,
    pathname: '/',
    hasPermission: () => true,
  })
  assert.ok(paths(selected).includes('/admin'))
})

test('path matching handles required and optional params', () => {
  assert.ok(pathMatchesCurrent('/series/:id', '/series/42'))
  assert.ok(pathMatchesCurrent('/read/:id/:chapter', '/read/42/7'))
  assert.ok(pathMatchesCurrent('/browse/:tag?', '/browse'))
  assert.ok(!pathMatchesCurrent('/series/:id', '/browse'))
})
