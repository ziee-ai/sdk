/**
 * TEST — `gallery.config.json` resolution + the unknown-key refusal.
 *
 * These exist because a blind auditor pointed out that the config validation and
 * the two new gate branches shipped with ZERO tests: the change added exactly one
 * test, and it was for a pre-existing note. Every case below is one the auditor
 * either reproduced or would have.
 *
 * Run: node --test scripts/lib/gallery-config.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { resolveGalleryConfig } from './gallery-config.mjs'

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
