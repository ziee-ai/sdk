import { test } from 'node:test'
import assert from 'node:assert/strict'
import { Fragment, isValidElement } from 'react'
import { Seam } from './Override.ts'
import { useOverride } from './useOverride.ts'
import { registerOverride, __clearOverrides } from './registry.ts'

// `Seam` and `useOverride` are pure resolution wrappers — we inspect the
// element they RETURN (its `.type`) rather than render to a DOM, which keeps the
// spec runnable under `node --test` (no testing-library). A real end-to-end
// render is covered by the e2e specs (TEST-9/10).
const KEY = 'test.seam' as never
const Fallback = () => null
const DesktopOverride = () => null

test('TEST-2: Seam renders the fallback (Fragment over children) when nothing is registered', () => {
  __clearOverrides()
  const el = Seam({ id: KEY, children: 'fallback' } as never) as ReturnType<
    typeof Seam
  >
  assert.ok(isValidElement(el))
  assert.equal((el as { type: unknown }).type, Fragment)
})

test('TEST-2: Seam renders the registered override when one is present', () => {
  __clearOverrides()
  registerOverride(KEY, DesktopOverride as never)
  const el = Seam({ id: KEY, children: 'fallback' } as never) as ReturnType<
    typeof Seam
  >
  assert.ok(isValidElement(el))
  assert.equal((el as { type: unknown }).type, DesktopOverride)
})

test('TEST-2: Seam forwards props to the override', () => {
  __clearOverrides()
  registerOverride(KEY, DesktopOverride as never)
  const el = Seam({ id: KEY, props: { a: 1 }, children: null } as never) as {
    props: unknown
  }
  assert.deepEqual(el.props, { a: 1 })
})

test('TEST-2: useOverride returns the fallback when unregistered, the override when registered', () => {
  __clearOverrides()
  assert.equal(useOverride(KEY, Fallback as never), Fallback)
  registerOverride(KEY, DesktopOverride as never)
  assert.equal(useOverride(KEY, Fallback as never), DesktopOverride)
})
