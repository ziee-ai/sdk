/**
 * `resolveVisualTier` — the guard that stands between a DECLARED-absent visual
 * tier and `playwright test -c null`.
 *
 * The regression this pins: `gate-ui.mjs` interpolated `CFG.visualConfig`
 * straight into the argv. `gallery.config.json` declares that field nullable
 * ("no visual tier"), so an honest `"visualConfig": null` became the literal
 * path `<uiRoot>/null`, playwright exited `Error: <uiRoot>/null does not exist`,
 * and the gate went red for an app that had correctly said it has no pixel tier.
 */
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { test } from 'node:test'
import { resolveGalleryConfig, resolveVisualTier } from './gallery-config.mjs'

/** A throwaway app root carrying the given `gallery.config.json` + extra files. */
function appDir(config, files = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gallery-cfg-'))
  fs.writeFileSync(path.join(dir, 'gallery.config.json'), JSON.stringify(config))
  for (const [name, body] of Object.entries(files))
    fs.writeFileSync(path.join(dir, name), body)
  return dir
}

test('a null visualConfig is a skip, never an argv path', () => {
  const dir = appDir({ visualConfig: null, visualSpecs: [] })
  const tier = resolveVisualTier(resolveGalleryConfig(dir))
  assert.equal(tier.enabled, false)
  assert.match(tier.reason, /no visual tier configured/)
  // The exact defect: nothing that could be spelled into `-c`.
  assert.equal(tier.config, undefined)
})

test('an empty visualSpecs is a skip even with a config present', () => {
  const dir = appDir(
    { visualConfig: 'playwright.visual.config.ts', visualSpecs: [] },
    { 'playwright.visual.config.ts': 'export default {}' },
  )
  const tier = resolveVisualTier(resolveGalleryConfig(dir))
  assert.equal(tier.enabled, false)
  assert.match(tier.reason, /visualSpecs: \[\]/)
})

test('a visualConfig naming a missing file is named, not handed to playwright', () => {
  const dir = appDir({ visualConfig: 'nope.config.ts', visualSpecs: ['layout.spec.ts'] })
  const tier = resolveVisualTier(resolveGalleryConfig(dir))
  assert.equal(tier.enabled, false)
  assert.match(tier.reason, /nope\.config\.ts does not exist/)
})

test('a configured tier resolves its config + specs', () => {
  const dir = appDir(
    {
      visualConfig: 'playwright.visual.config.ts',
      visualSpecs: ['layout.spec.ts', 'states.spec.ts'],
      visualSnapshotSpecs: ['gallery.spec.ts'],
    },
    { 'playwright.visual.config.ts': 'export default {}' },
  )
  const cfg = resolveGalleryConfig(dir)
  const tier = resolveVisualTier(cfg)
  assert.deepEqual(tier, {
    enabled: true,
    config: 'playwright.visual.config.ts',
    specs: ['layout.spec.ts', 'states.spec.ts'],
  })
  assert.deepEqual(resolveVisualTier(cfg, { snapshots: true }).specs, [
    'layout.spec.ts',
    'states.spec.ts',
    'gallery.spec.ts',
  ])
})

test('an app shipping no gallery.config.json keeps the historical defaults', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gallery-cfg-'))
  fs.writeFileSync(path.join(dir, 'playwright.visual.config.ts'), 'export default {}')
  const tier = resolveVisualTier(resolveGalleryConfig(dir))
  assert.equal(tier.enabled, true)
  assert.deepEqual(tier.specs, ['layout.spec.ts', 'states.spec.ts', 'overlays.spec.ts'])
})
