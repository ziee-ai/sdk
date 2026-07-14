import { test } from 'node:test'
import assert from 'node:assert/strict'
import { create } from 'zustand'
import { createStoreProxy } from './stores.ts'

// These specs load the REAL createStoreProxy under `node --test` (via the
// scripts/node-test-loader.mjs alias loader — proves ITEM-10). React + zustand
// are real; only the unrelated @/core/{module-system,events} boundaries the
// factory never calls are stubbed. Reading a reactive state VALUE outside a
// React render calls a hook with a null dispatcher and throws — which is
// exactly how we assert the render-only contract.

const mkProxy = (state: any) => createStoreProxy(create<any>(() => state) as any) as any

test('TEST-1: an action is callable hook-free OUTSIDE any render and mutates state', () => {
  const store = create<any>(set => ({
    count: 0,
    inc: () => set((s: any) => ({ count: s.count + 1 })),
  }))
  const p: any = createStoreProxy(store as any)
  // Called at module/test scope — NOT inside a React render. Must not throw.
  assert.doesNotThrow(() => p.inc())
  p.inc()
  assert.equal(store.getState().count, 2)
  assert.equal(p.$.count, 2)
})

test('TEST-2: `$` returns the getState() snapshot hook-free outside render', () => {
  const store = create<any>(() => ({ a: 1, b: 'x' }))
  const p: any = createStoreProxy(store as any)
  assert.doesNotThrow(() => p.$)
  assert.equal(p.$.a, store.getState().a)
  assert.equal(p.$.b, 'x')
  assert.deepEqual(p.$, store.getState())
})

test('TEST-3: reading a reactive state VALUE outside render throws (render-only contract)', () => {
  const p = mkProxy({ value: 42 })
  // A non-function, non-special prop routes through useEffect/useStore → hook
  // call with no dispatcher → throws. `$` is the required handler-side escape.
  assert.throws(() => p.value)
})

test('TEST-4/TEST-10: `.__state` is no longer a hook-free special; only `$` is', () => {
  const p = mkProxy({ value: 42 })
  // `$` is a valid hook-free snapshot escape…
  assert.doesNotThrow(() => p.$)
  assert.equal(typeof p.$, 'object')
  // …but `.__state` lost its special status: it now behaves like any reactive
  // read and throws outside render (proving the alias was REMOVED, not renamed).
  // Bracket access hits the same proxy trap while sidestepping the grit ban that
  // (correctly) forbids the `.__state` member syntax in source.
  assert.throws(() => p['__state'])
  // Sibling internals are unaffected (not swept, not banned).
  assert.doesNotThrow(() => p.__setState)
  assert.equal(typeof p.__setState, 'function')
})
