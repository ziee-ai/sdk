/**
 * TEST-6, TEST-7 — the pure registry logic (ITEM-2, ITEM-3): cassette merge with
 * collision-throw, and slug-uniqueness across surface classes.
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  type DiscoveredGallery,
  assertUniqueSlugs,
  mergeModuleCassettes,
  moduleNameFromPath,
} from './registry-core.ts'

const g = (module: string, gallery: any): DiscoveredGallery => ({ module, gallery })

// ── TEST-6: mergeModuleCassettes ─────────────────────────────────────────────
test('TEST-6: module cassettes merge into one map', () => {
  const merged = mergeModuleCassettes([
    g('a', { cassette: { 'Foo.list': { items: [1] } } }),
    g('b', { cassette: { 'Bar.get': { id: 'x' } } }),
  ]) as any
  assert.deepEqual(merged['Foo.list'], { items: [1] })
  assert.deepEqual(merged['Bar.get'], { id: 'x' })
})

test('TEST-6: a duplicate endpoint key across two modules THROWS', () => {
  assert.throws(
    () =>
      mergeModuleCassettes([
        g('a', { cassette: { 'Foo.list': {} } }),
        g('b', { cassette: { 'Foo.list': {} } }),
      ]),
    /cassette collision on "Foo\.list".*"a".*"b"/,
  )
})

test('TEST-6: a module with no cassette contributes nothing', () => {
  const merged = mergeModuleCassettes([g('a', { crawlOnly: true })]) as any
  assert.deepEqual(Object.keys(merged), [])
})

// ── TEST-7: assertUniqueSlugs ────────────────────────────────────────────────
test('TEST-7: distinct slugs across classes pass', () => {
  assert.doesNotThrow(() =>
    assertUniqueSlugs([
      g('a', { overlays: [{ slug: 'o1' }], seeded: [{ slug: 's1' }] }),
      g('b', { deepStates: [{ slug: 'd1' }] }),
    ]),
  )
})

test('TEST-7: a duplicate slug across classes/modules THROWS', () => {
  assert.throws(
    () =>
      assertUniqueSlugs([
        g('a', { overlays: [{ slug: 'dup' }] }),
        g('b', { seeded: [{ slug: 'dup' }] }),
      ]),
    /duplicate surface slug "dup"/,
  )
})

// ── moduleNameFromPath ───────────────────────────────────────────────────────
test('moduleNameFromPath extracts the module dir', () => {
  assert.equal(
    moduleNameFromPath('../../../modules/knowledge-base/gallery.tsx'),
    'knowledge-base',
  )
})
