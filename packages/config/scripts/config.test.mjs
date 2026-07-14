/**
 * Smoke test for @ziee/config: the pure configs resolve, the syncpack helper
 * composes, and a parameterized lint runs against an arbitrary `--root` dir
 * (the parameterization proof) — pass on clean input, fail on a violation.
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import os from 'node:os'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const PKG = path.resolve(HERE, '..')
const node = process.execPath

test('biome.base.json is valid JSON with the generic linter preset', () => {
  const j = JSON.parse(fs.readFileSync(path.join(PKG, 'biome.base.json'), 'utf-8'))
  assert.equal(j.linter.rules.recommended, false)
  assert.ok(j.formatter.enabled)
  // The app-specific antd noRestrictedImports must NOT be in the base preset.
  const str = JSON.stringify(j)
  assert.ok(!str.includes('noRestrictedImports'), 'base must not hard-code app import bans')
})

test('tsconfig.base.json exposes strict generic compilerOptions (no app paths)', () => {
  const raw = fs.readFileSync(path.join(PKG, 'tsconfig.base.json'), 'utf-8')
  assert.ok(raw.includes('"strict": true'))
  assert.ok(!raw.includes('"paths"'), 'base must not hard-code app path mappings')
})

test('defineSyncpack composes semver + version groups (catch-all last)', async () => {
  const { defineSyncpack, semverGroups } = await import(path.join(PKG, 'syncpack.base.mjs'))
  const cfg = defineSyncpack({ source: ['package.json'], versionGroups: [{ label: 'x', dependencies: ['a'], packages: ['**'] }] })
  assert.equal(cfg.semverGroups.length, 2)
  assert.equal(semverGroups[0].dependencies[0], 'typescript')
  assert.equal(cfg.versionGroups.at(-1).policy, 'sameRange')
  assert.equal(cfg.versionGroups.length, 2)
})

test('a parameterized lint scans an arbitrary --root: clean passes, violation fails', () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'ziee-config-lint-'))
  const colors = path.join(PKG, 'src/lint/hardcoded-colors.mjs')
  // clean file — semantic token class only
  fs.writeFileSync(path.join(dir, 'ok.tsx'), `export const A = () => <div className="bg-primary" />\n`)
  let r = spawnSync(node, [colors, `--root=${dir}`], { encoding: 'utf-8' })
  assert.equal(r.status, 0, r.stdout + r.stderr)
  // violation — raw Tailwind hue
  fs.writeFileSync(path.join(dir, 'bad.tsx'), `export const B = () => <div className="bg-blue-500" />\n`)
  r = spawnSync(node, [colors, `--root=${dir}`], { encoding: 'utf-8' })
  assert.equal(r.status, 1)
  assert.ok((r.stdout + r.stderr).includes('bg-blue-500'))
  fs.rmSync(dir, { recursive: true, force: true })
})
