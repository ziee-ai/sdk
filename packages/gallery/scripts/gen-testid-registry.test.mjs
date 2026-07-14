/**
 * TEST — the testid-registry generator's pure core: literal extraction (static
 * `data-testid=`/`testid:` only, never derived template ids), source-file
 * selection (skip gallery seeds + generated output + the dev gallery), and the
 * byte-stable render.
 * Run: node --test scripts/gen-testid-registry.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {
  collectSourceFiles,
  collectTestIds,
  renderRegistry,
} from './gen-testid-registry.mjs'

test('collectTestIds extracts static data-testid literals (= and : forms)', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  fs.writeFileSync(
    path.join(dir, 'A.tsx'),
    `<div data-testid="alpha" />\n  <b data-testid='gamma' />\n  const o = { 'data-testid': 'beta' }`,
  )
  const ids = collectTestIds([path.join(dir, 'A.tsx')])
  // `=` (double + single quote) forms captured; the quoted-key object form
  // (`'data-testid':`) is NOT — mirrors the original app generator's regex.
  assert.deepEqual([...ids].sort(), ['alpha', 'gamma'])
  fs.rmSync(dir, { recursive: true, force: true })
})

test('collectTestIds ignores derived/template ids (no static literal)', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  fs.writeFileSync(
    path.join(dir, 'B.tsx'),
    'data-testid={`${row}-cell`} data-testid={someVar} data-testid="kept"',
  )
  const ids = collectTestIds([path.join(dir, 'B.tsx')])
  assert.deepEqual([...ids], ['kept'])
  fs.rmSync(dir, { recursive: true, force: true })
})

test('collectSourceFiles skips gallery seeds, generated output, tests + src/dev', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  const src = path.join(root, 'src')
  fs.mkdirSync(path.join(src, 'dev', 'gallery'), { recursive: true })
  fs.mkdirSync(path.join(src, 'tests'), { recursive: true })
  fs.mkdirSync(path.join(src, 'modules'), { recursive: true })
  fs.writeFileSync(path.join(src, 'Keep.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'testIds.generated.ts'), 'x')
  fs.writeFileSync(path.join(src, 'modules', 'gallery.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'dev', 'gallery', 'Story.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'tests', 'a.ts'), 'x')
  const got = collectSourceFiles(src).map((f) => path.basename(f))
  assert.deepEqual(got, ['Keep.tsx'])
  fs.rmSync(root, { recursive: true, force: true })
})

test('renderRegistry is deterministic + emits the KnownTestId union', () => {
  const body = renderRegistry(['a-btn', 'b-btn'])
  assert.match(body, /export const TEST_IDS = \[\n {2}"a-btn",\n {2}"b-btn",\n\] as const/)
  assert.match(body, /export type KnownTestId = \(typeof TEST_IDS\)\[number\]/)
  assert.match(body, /export const isKnownTestId/)
  // 2 ids → the header count reflects the input length.
  assert.match(body, /2 static data-testid ids/)
})
