import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createAppStoreSeam } from './app-seam.ts'

// The app-store injection seam has TWO reads: `get()` is the loud, boot-critical
// read (throws if the app never injected the store), `peek()` is the non-throwing
// optional read for consumers that legitimately render in a store-LESS context
// (an isolated gallery overlay, a layout-less route) and degrade gracefully.
// This is the regression guard for the "AppLayout store was not registered"
// crash class: the optional shell chrome reads via `peek()`, which must return
// null (never throw) before injection.

test('get() throws before injection', () => {
  const seam = createAppStoreSeam<{ v: number }>('Test')
  assert.throws(() => seam.get(), /was not registered/)
})

test('peek() returns null before injection (never throws)', () => {
  const seam = createAppStoreSeam<{ v: number }>('Test')
  assert.equal(seam.peek(), null)
})

test('peek() and get() both return the injected view after set()', () => {
  const seam = createAppStoreSeam<{ v: number }>('Test')
  const view = { v: 42 }
  seam.set(view)
  assert.equal(seam.peek(), view)
  assert.equal(seam.get(), view)
  // Reference identity is preserved (so a destructured reactive field still
  // subscribes) — peek returns the very same object get would.
  assert.equal(seam.peek(), seam.get())
})
