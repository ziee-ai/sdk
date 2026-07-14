import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  registerOverride,
  resolveOverride,
  __clearOverrides,
  __registeredOverrideKeys,
} from './registry.ts'

// The registry is keyed by declared `UIOverrides` seams; in this isolated spec
// no seam is declared, so we cast a test key through `any` to exercise the
// runtime Map behavior (the TYPE enforcement is proven separately by `tsc`).
const KEY = 'test.button' as never
const Comp = () => null
const Comp2 = () => null

test('TEST-1: resolveOverride returns undefined for an unregistered key', () => {
  __clearOverrides()
  assert.equal(resolveOverride(KEY), undefined)
})

test('TEST-1: registerOverride then resolveOverride returns the component', () => {
  __clearOverrides()
  registerOverride(KEY, Comp as never)
  assert.equal(resolveOverride(KEY), Comp)
  assert.deepEqual(__registeredOverrideKeys(), ['test.button'])
})

test('TEST-1: re-registering a key is last-write-wins', () => {
  __clearOverrides()
  registerOverride(KEY, Comp as never)
  registerOverride(KEY, Comp2 as never)
  assert.equal(resolveOverride(KEY), Comp2)
})

test('TEST-1: __clearOverrides empties the registry', () => {
  registerOverride(KEY, Comp as never)
  __clearOverrides()
  assert.deepEqual(__registeredOverrideKeys(), [])
})
