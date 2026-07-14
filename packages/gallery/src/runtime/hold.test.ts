/**
 * TEST-8 — the store-seed durability helpers a per-module `gallery.tsx` needs
 * (ITEM-1). The `support/` barrel re-exports these + the lazy-render helpers +
 * the entry types; its VALUE re-exports + type surface are verified by tsc
 * (`npm run check`). Here we exercise the JSX-free runtime helpers directly (the
 * Node test loader transpiles `.ts`, not the `.tsx` lazy helpers).
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { holdForever, holdPatch, whenTrue } from './hold.ts'

test('TEST-8: the durability helpers are exported functions', () => {
  for (const fn of [holdForever, holdPatch, whenTrue]) {
    assert.equal(typeof fn, 'function')
  }
})

test('TEST-8: whenTrue resolves immediately when the predicate is already true', async () => {
  let calls = 0
  await whenTrue(() => {
    calls++
    return true
  })
  assert.equal(calls, 1)
})

test('TEST-8: holdPatch applies the patch the requested number of times', async () => {
  let n = 0
  await holdPatch(() => n++, 3, 1)
  assert.equal(n, 3)
})
