/**
 * TEST — the config-driven gate branches, DRIVEN FOR REAL.
 *
 * `visualConfig: null` and the `gateExtraCmds` loop shipped with zero coverage:
 * the only existing gate e2e writes `visualConfig: 'none'` (truthy, so the new
 * branch never runs) and never sets `gateExtraCmds`. The cwd guard had none
 * either. A blind auditor had to build these fixtures to check the change at all,
 * which is a fair sign they belonged in the diff.
 *
 * These spawn the REAL scripts. They do not boot Vite: every case is decided
 * before the dev-server phase, or by the cwd guard, so each run is fast.
 *
 * Run: node --test scripts/gate-ui.config.e2e.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawnSync } from 'node:child_process'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const GATE = path.join(HERE, 'gate-ui.mjs')
const CRAWL = path.join(HERE, 'runtime-health.mjs')

const runIn = (cwd, script, args = [], timeout = 60_000) =>
  spawnSync(process.execPath, [script, ...args], { cwd, encoding: 'utf8', timeout })

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'gate-cfg-'))

// ---------------------------------------------------------------------------
// The cwd guard. These scripts are shared by several workspaces and addressed by
// a long relative path, so running them from the wrong directory is easy — and
// without a config every anchor silently falls back to another app's defaults,
// including portWhich (i.e. another gallery's port).
// ---------------------------------------------------------------------------
for (const [name, script] of [
  ['gate-ui', GATE],
  ['runtime-health', CRAWL],
]) {
  test(`${name} REFUSES to run with no gallery.config.json in cwd`, () => {
    const dir = tmp()
    try {
      const r = runIn(dir, script)
      assert.equal(r.status, 2, `expected exit 2, got ${r.status}\n${r.stdout}${r.stderr}`)
      const out = `${r.stdout}${r.stderr}`
      assert.match(out, /refusing to run/, 'must say it refused')
      assert.match(out, /gallery\.config\.json/, 'must name what is missing')
      assert.doesNotMatch(
        out,
        /at Object\.|at Module\._compile|\n\s+at /,
        'must be an operator message, not a stack trace',
      )
    } finally {
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

  test(`${name} does NOT refuse when a config IS present`, () => {
    // The positive control that stops "REFUSES" from meaning "always fails".
    // Asserted on the MESSAGE, not the exit code: past the guard these scripts go
    // on to boot a dev server / probe an origin, so a status assertion would
    // either hang or encode an unrelated later failure. Reaching that later work
    // IS the proof the guard let it through.
    const dir = tmp()
    try {
      fs.writeFileSync(path.join(dir, 'gallery.config.json'), '{}')
      const r = runIn(dir, script, ['--no-wait'], 20_000)
      assert.doesNotMatch(
        `${r.stdout}${r.stderr}`,
        /refusing to run — no gallery\.config\.json/,
        'a config IS present, so the cwd guard must not fire',
      )
    } finally {
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

}

test('an unknown config key stops the gate, naming the key', () => {
  // The end-to-end face of the unit case: a typo must not reach the crawl.
  const dir = tmp()
  try {
    fs.writeFileSync(
      path.join(dir, 'gallery.config.json'),
      JSON.stringify({ gateExtraCommands: [['coverage', 'true', []]] }),
    )
    const r = runIn(dir, GATE)
    assert.notEqual(r.status, 0, 'a typo must never yield a green gate')
    assert.match(`${r.stdout}${r.stderr}`, /unknown key\(s\): gateExtraCommands/)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test('DOC-DRIFT only: the gateExtraCmds doc states the shape the gate destructures', () => {
  // Scope, stated plainly because a source-text assertion standing in for a
  // behavioural one is the mistake this branch keeps paying for: this checks ONLY
  // that the doc and the destructuring agree. It is NOT evidence the stage works.
  //
  // The BEHAVIOURAL proof is desktop's real `gate:ui` run, which executed a
  // config-declared 3-tuple stage end-to-end (`• coverage …` / `✅ coverage — ok`)
  // — recorded as TEST-38. This case exists only because F4 was a doc that
  // contradicted the code: following "same shape as lintCmds" (a 2-tuple) yields
  // label='npm', cmd=['run','x'] and an opaque `The "file" argument must be of
  // type string`.
  const src = fs.readFileSync(path.join(HERE, 'gate-ui.mjs'), 'utf8')
  assert.match(
    src,
    /for \(const \[label, cmd, args\] of CFG\.gateExtraCmds/,
    'the gate must destructure a 3-tuple',
  )
  const cfgSrc = fs.readFileSync(path.join(HERE, 'lib/gallery-config.mjs'), 'utf8')
  // The doc comment sits ABOVE the key, so window BACKWARDS from the key rather
  // than forwards from it (slicing forwards starts after the doc and matches
  // nothing — which made this assertion vacuous in its first form).
  const at = cfgSrc.indexOf('gateExtraCmds:')
  assert.ok(at > 0, 'gateExtraCmds must exist in DEFAULTS')
  const doc = cfgSrc.slice(Math.max(0, at - 900), at)
  assert.match(doc, /\[label, cmd, args\]/, 'the config doc must state that shape')
  assert.doesNotMatch(
    doc,
    /Same shape as `lintCmds`/,
    'the doc must NOT claim lintCmds shape — that reading crashes',
  )
})
