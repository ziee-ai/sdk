#!/usr/bin/env node
/**
 * Composable quality-gate runner: `tsc + biome guardrail + design-token
 * enforcement + hardcoded-color + design-spec (+ kit-manifest)` in ONE call, so a
 * new app gets ziee's whole config-layer gate by referencing this instead of
 * hand-maintaining a 10-step `&&` chain.
 *
 * Everything is parameterized over the app's tree (defaults assume an app root with
 * `src/`, `src/index.css`, `tsconfig.json`, `DESIGN_SYSTEM.md`):
 *
 *   ziee-check                                  # all steps, defaults
 *   ziee-check --root=src --root=../desktop/ui/src
 *   ziee-check --css src/index.css --design-out ../../DESIGN_SYSTEM.md
 *   ziee-check --kit-barrel ../../sdk/packages/kit/src/index.ts --kit-out .../KIT_MANIFEST.md
 *   ziee-check --no-tsc --no-kit-manifest       # skip steps
 *
 * NOTE: this is the CONFIG-layer gate only. Gallery/visual checks (gallery-coverage,
 * state-matrix, runtime-health, …) belong to a separate @ziee/gallery layer and are
 * NOT run here.
 */
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import { parseMulti, parseOne } from './lint/roots.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const LINT = p => path.join(HERE, 'lint', p)
const argv = process.argv.slice(2)
const has = f => argv.includes(f)

const roots = parseMulti('root')
const rootArgs = (roots.length ? roots : ['src']).flatMap(r => [`--root=${r}`])
const css = parseOne('css') ?? 'src/index.css'
const designOut = parseOne('design-out') ?? 'DESIGN_SYSTEM.md'
const kitBarrel = parseOne('kit-barrel') ?? 'src/components/ui/index.ts'
const kitOut = parseOne('kit-out')

const node = process.execPath
const steps = []
if (!has('--no-tsc')) steps.push(['tsc', 'npx', ['--no-install', 'tsc', '--noEmit']])
if (!has('--no-biome'))
  steps.push([
    'biome:guardrail',
    'npx',
    ['--no-install', 'biome', 'lint', '--only=style/noRestrictedImports', ...(roots.length ? roots : ['src'])],
  ])
if (!has('--no-colors')) steps.push(['lint:colors', node, [LINT('hardcoded-colors.mjs'), ...rootArgs]])
if (!has('--no-settings-field'))
  steps.push(['lint:settings-field', node, [LINT('settings-field.mjs'), ...rootArgs]])
if (!has('--no-adjacent-inline'))
  steps.push(['lint:adjacent-inline', node, [LINT('adjacent-inline.mjs'), ...rootArgs]])
if (!has('--no-logical-direction'))
  steps.push(['lint:logical-direction', node, [LINT('logical-direction.mjs')]])
if (!has('--no-tooltip-placement'))
  steps.push(['lint:tooltip-placement', node, [LINT('tooltip-placement.mjs'), ...rootArgs]])
if (!has('--no-design-spec'))
  steps.push(['check:design-spec', node, [LINT('design-spec.mjs'), '--css', css, '--out', designOut, '--check']])
if (!has('--no-kit-manifest') && fs.existsSync(path.resolve(process.cwd(), kitBarrel))) {
  const a = [LINT('kit-manifest.mjs'), '--barrel', kitBarrel]
  if (kitOut) a.push('--out', kitOut)
  a.push('--check')
  steps.push(['check:kit-manifest', node, a])
}

let failed = 0
for (const [name, cmd, args] of steps) {
  process.stdout.write(`\n▶ ${name}\n`)
  const r = spawnSync(cmd, args, { stdio: 'inherit', cwd: process.cwd() })
  if (r.status !== 0) {
    failed++
    console.error(`✗ ${name} failed (exit ${r.status ?? 'signal'})`)
  }
}
if (failed) {
  console.error(`\n[ziee-check] ✗ ${failed} step(s) failed.`)
  process.exit(1)
}
console.log(`\n[ziee-check] ✓ all ${steps.length} config-gate step(s) passed.`)
