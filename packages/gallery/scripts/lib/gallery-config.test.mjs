/**
 * TEST — `gallery.config.json` resolution + `resolveVisualTier` + the unknown-key refusal.
 *
 * Two complementary suites over one module:
 *  - `resolveVisualTier` — the guard between a DECLARED-absent visual tier and
 *    `playwright test -c null`. `gate-ui.mjs` interpolated `CFG.visualConfig`
 *    straight into the argv, so an honest `"visualConfig": null` became the
 *    literal path `<uiRoot>/null` and the gate went red for an app that had
 *    correctly said it has no pixel tier.
 *  - `resolveGalleryConfig` — config resolution + the unknown-key refusal (a typo
 *    like `gateExtraCommands` silently deleted a whole gate stage), plus that the
 *    package's OWN documented visual keys (visualTestDir/maxDiffPixelRatio) are
 *    accepted.
 *
 * Run: node --test scripts/lib/gallery-config.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
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

const withConfig = (obj, fn) => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gallery-cfg-'))
  try {
    if (obj !== null)
      fs.writeFileSync(path.join(dir, 'gallery.config.json'), JSON.stringify(obj, null, 2))
    return fn(dir)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

test('an unknown key is REFUSED, naming the key', () => {
  // The defect this exists for: `gateExtraCommands` (a typo of gateExtraCmds)
  // silently deleted a whole gate stage — no line printed, exit 0.
  withConfig({ gateExtraCommands: [] }, dir => {
    assert.throws(() => resolveGalleryConfig(dir), /unknown key\(s\): gateExtraCommands/)
  })
})

test('prototype-chain names are REFUSED, not silently accepted', () => {
  // `k in DEFAULTS` walks the prototype, so these were all ACCEPTED: a typo
  // landing on one is silently ignored (the exact defect the check exists to
  // stop), and an own `toString` would shadow the method on the merged object.
  // NOTE: `__`-prefixed names are a DELIBERATE escape (the resolver's own
  // `__cwd`/`__configPath` live there), so they are allowed by design and are
  // not part of this case.
  for (const k of ['constructor', 'toString', 'hasOwnProperty', 'valueOf'])
    withConfig({ [k]: 'x' }, dir => {
      assert.throws(
        () => resolveGalleryConfig(dir),
        /unknown key/,
        `"${k}" must be refused, not inherited from Object.prototype`,
      )
    })
})

test('the visual-layer keys this package itself documents are ACCEPTED', () => {
  // `playwright/visual.config.ts` in THIS package reads visualTestDir and
  // maxDiffPixelRatio. A validator refusing its own package's documented contract
  // is the "shared tooling assumes one consumer" defect, and it broke every
  // script that resolves config (i.e. most of `npm run check`).
  withConfig({ visualTestDir: './tests/e2e/visual', maxDiffPixelRatio: 0.02 }, dir => {
    const c = resolveGalleryConfig(dir)
    assert.equal(c.visualTestDir, './tests/e2e/visual')
    assert.equal(c.maxDiffPixelRatio, 0.02)
  })
})

test('$-prefixed comment keys are allowed, and the error says so', () => {
  withConfig({ $comment: 'hi', $schema: 'x' }, dir => {
    assert.doesNotThrow(() => resolveGalleryConfig(dir))
  })
  withConfig({ nope: 1 }, dir => {
    assert.throws(() => resolveGalleryConfig(dir), /prefix the key with "\$"/)
  })
})

test('every real key is accepted, and defaults survive a partial config', () => {
  withConfig({ portWhich: 'desktopGallery' }, dir => {
    const c = resolveGalleryConfig(dir)
    assert.equal(c.portWhich, 'desktopGallery')
    assert.equal(c.galleryUrl, '/gallery.html', 'unset keys keep their default')
  })
})

test('visualConfig: null is a legal value (an app with no visual layer)', () => {
  withConfig({ visualConfig: null }, dir => {
    assert.equal(resolveGalleryConfig(dir).visualConfig, null)
  })
})

test('a missing config file still resolves to defaults (only the two entry points refuse)', () => {
  withConfig(null, dir => {
    const c = resolveGalleryConfig(dir)
    assert.equal(c.galleryDir, 'src/dev/gallery')
    assert.equal(fs.existsSync(c.__configPath), false)
  })
})

test('malformed JSON throws naming the file', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'gallery-cfg-'))
  try {
    fs.writeFileSync(path.join(dir, 'gallery.config.json'), '{ not json')
    assert.throws(() => resolveGalleryConfig(dir), /failed to parse/)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
