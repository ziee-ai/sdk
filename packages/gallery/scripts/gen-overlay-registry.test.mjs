/**
 * TEST-5 — the overlay-registry gate discovers `surface:` from per-module
 * gallery.tsx, not only overlays.tsx (ITEM-18).
 * Run: node --test scripts/gen-overlay-registry.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { extractWiredSurfaces } from './gen-overlay-registry.mjs'

test('TEST-5: extractWiredSurfaces picks up surface: from a per-module gallery.tsx', () => {
  const moduleGallery = `
    export const gallery = {
      overlays: [
        { slug: 'overlay-foo', surface: 'modules/foo/FooDrawer', title: 'Foo', component: X },
      ],
    }
  `
  const centralAggregator = `export { OVERLAY_ENTRIES } from './support/registry'`
  const wired = extractWiredSurfaces([centralAggregator, moduleGallery])
  assert.ok(wired.has('modules/foo/FooDrawer'))
})

test('TEST-5: multiple sources union their surfaces', () => {
  const a = `surface: 'modules/a/A'`
  const b = `surface: 'modules/b/B'`
  const wired = extractWiredSurfaces([a, b])
  assert.deepEqual([...wired].sort(), ['modules/a/A', 'modules/b/B'])
})

test('TEST-5: a thin aggregator with no surface: fields contributes nothing', () => {
  const wired = extractWiredSurfaces([`export { OVERLAY_ENTRIES } from './x'`])
  assert.equal(wired.size, 0)
})
