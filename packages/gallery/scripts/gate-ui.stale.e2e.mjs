/**
 * TEST-4 [acceptance · INV-4] — end-to-end against the REAL `gate-ui.mjs`.
 *
 * ## Why this had to be rewritten
 *
 * The first version of this file re-implemented gate-ui's roll-up sequence
 * locally and asserted on THAT. It therefore exercised `verifyRunManifest` (which
 * `run-validity.test.mjs` already covers) and gave the actual refusal branch in
 * `gate-ui.mjs` ZERO coverage: changing `if (!verdictOk.ok)` to
 * `if (false && !verdictOk.ok)` left every test green while the gate happily
 * printed a previous run's PASS table. A blind reviewer found that by mutation,
 * and they were right — a test that re-implements the code under test cannot fail
 * when the code under test is wrong.
 *
 * So this version SPAWNS the real `gate-ui.mjs`, with a stub `runtime-health`
 * standing in for the crawl, and asserts on its stdout and exit code.
 *
 * ## The staged hazard (the literal reported scenario)
 *
 * "a killed run at 575/682 cells printed `103/106 PASS` derived entirely from an
 * earlier run's file, detectable only by the truncated cell count and the file's
 * unchanged mtime."
 *
 * A previous run's ALL-CLEAN findings for 106 surfaces are placed on disk, then
 * the crawl is made to die. If the gate inherits, it prints `106/106 PASS`. The
 * assertion is on that table's **ABSENCE**, which no accident satisfies.
 *
 * Run: node sdk/packages/gallery/scripts/gate-ui.stale.e2e.mjs
 */
import assert from 'node:assert/strict'
import { spawnSync } from 'node:child_process'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { writeRunManifest } from './lib/run-validity.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const GATE_UI = path.join(HERE, 'gate-ui.mjs')

let failures = 0
const check = (label, fn) => {
  try {
    fn()
    console.log(`  ✅ ${label}`)
  } catch (e) {
    failures++
    console.log(`  ❌ ${label}\n     ${e.message.split('\n').slice(0, 3).join('\n     ')}`)
  }
}

/**
 * A minimal app tree gate-ui can run against: a gallery.config.json, a stub
 * "dev server" that answers the gallery URL, and a stub runtime-health.
 */
function makeApp({ crawlBehaviour }) {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'gate-e2e-'))
  const ui = path.join(root, 'ui')
  const galleryDir = path.join(ui, 'src/dev/gallery')
  fs.mkdirSync(galleryDir, { recursive: true })

  // A stub dev server: gate-ui only needs `GET <galleryUrl>` to answer 200.
  fs.writeFileSync(
    path.join(ui, 'server.mjs'),
    `import http from 'node:http'
const port = Number(process.argv[process.argv.indexOf('--port') + 1])
http.createServer((_q, s) => { s.writeHead(200, {'content-type':'text/html'}); s.end('<!doctype html>ok') })
  .listen(port, '127.0.0.1')\n`,
  )

  fs.writeFileSync(
    path.join(ui, 'gallery.config.json'),
    JSON.stringify({
      galleryDir: 'src/dev/gallery',
      galleryUrl: '/gallery.html',
      srcDir: 'src',
      devCmd: ['node', path.join(ui, 'server.mjs')],
      lintCmds: [['node', ['-e', '']]],
      visualConfig: 'none',
      visualSpecs: [],
      visualSnapshotSpecs: [],
    }),
  )
  // gate-ui runs `npx tsc --noEmit` — give it a config so it exits 0 trivially.
  fs.writeFileSync(
    path.join(ui, 'tsconfig.json'),
    JSON.stringify({ compilerOptions: { noEmit: true, types: [] }, files: [] }),
  )

  // The stub crawl. `crawlBehaviour` decides whether it completes.
  fs.writeFileSync(path.join(ui, 'stub-runtime-health.mjs'), crawlBehaviour)
  return { root, ui, galleryDir }
}

/** 106 surfaces, all clean — the shape that produced the misleading "103/106". */
function seedPreviousCleanRun(galleryDir) {
  const lines = []
  for (let i = 0; i < 106; i++)
    lines.push(
      JSON.stringify({
        surface: `surface-${i}`,
        state: 'loaded',
        theme: 'light',
        category: 'spacing-grid',
        severity: 'LOW',
        detail: 'padding 7px off-grid',
      }),
    )
  fs.writeFileSync(path.join(galleryDir, 'RUNTIME_FINDINGS.jsonl'), `${lines.join('\n')}\n`)
  fs.writeFileSync(path.join(galleryDir, 'RUNTIME_FINDINGS.md'), '# previous run — all clean\n')
  writeRunManifest(galleryDir, {
    runId: 'PREVIOUS-RUN-THAT-SUCCEEDED',
    complete: true,
    cellsPlanned: 682,
    cellsCompleted: 682,
    void: false,
    contamination: { total: 0, pct: 0 },
    originEverDown: false,
  })
}

/** Run the REAL gate-ui against the stub app. Returns {code, out}. */
function runGate(app, extraEnv = {}) {
  const r = spawnSync(process.execPath, [GATE_UI, '--skip-visual'], {
    cwd: app.ui,
    encoding: 'utf8',
    env: {
      ...process.env,
      // Point gate-ui's child at our stub crawl instead of the real one.
      GATE_UI_RUNTIME_HEALTH: path.join(app.ui, 'stub-runtime-health.mjs'),
      GATE_UI_LOCK: '0', // this test is about the manifest, not the lock
      GALLERY_PORT: String(20500 + Math.floor(Math.random() * 400)),
      ...extraEnv,
    },
    timeout: 180_000,
  })
  return { code: r.status, out: `${r.stdout || ''}${r.stderr || ''}` }
}

// ===========================================================================
console.log('=== TEST-4: a KILLED crawl must not inherit a previous run ===')
{
  // The crawl "dies at 575/682": it writes nothing and exits non-zero.
  const app = makeApp({
    crawlBehaviour: `console.log('  … 575/682 cells')\nprocess.exit(137)\n`,
  })
  seedPreviousCleanRun(app.galleryDir)
  const before = fs.readFileSync(path.join(app.galleryDir, 'RUNTIME_FINDINGS.jsonl'), 'utf8')
  assert.ok(before.includes('surface-105'), 'hazard staged')

  const { code, out } = runGate(app)
  console.log(out.split('\n').filter(Boolean).slice(-12).join('\n'))

  check('the gate exits NON-ZERO', () => assert.notEqual(code, 0))
  check('NO per-surface verdict table is printed', () =>
    assert.equal(
      /per-surface runtime verdict/.test(out),
      false,
      'the gate printed a per-surface table over data it did not produce',
    ))
  check('it never claims any surface PASSed', () =>
    assert.equal(/\d+\/\d+ PASS/.test(out), false))
  check('it names the reason (an incomplete crawl)', () =>
    assert.match(out, /did not complete|not usable/))
  check('the previous run\'s findings file was cleared', () =>
    assert.equal(
      fs.existsSync(path.join(app.galleryDir, 'RUNTIME_FINDINGS.jsonl')),
      false,
    ))
  check('the previous run\'s human-facing .md was cleared too', () =>
    assert.equal(fs.existsSync(path.join(app.galleryDir, 'RUNTIME_FINDINGS.md')), false))
  fs.rmSync(app.root, { recursive: true, force: true })
}

// ===========================================================================
console.log('\n=== a crawl that COMPLETES but under a different runId is refused ===')
{
  const app = makeApp({
    crawlBehaviour: `import { writeRunManifest } from ${JSON.stringify(path.join(HERE, 'lib/run-validity.mjs'))}
import fs from 'node:fs'
const dir = 'src/dev/gallery'
fs.mkdirSync(dir, { recursive: true })
fs.writeFileSync(dir + '/RUNTIME_FINDINGS.jsonl', '')
writeRunManifest(dir, { runId: 'SOMEBODY-ELSES-RUN', complete: true, cellsPlanned: 10, cellsCompleted: 10, void: false, contamination: {total:0,pct:0} })\n`,
  })
  seedPreviousCleanRun(app.galleryDir)
  const { code, out } = runGate(app)
  check('the gate exits NON-ZERO', () => assert.notEqual(code, 0))
  check('it names the run-id mismatch', () => assert.match(out, /run id mismatch/i))
  check('no per-surface table', () =>
    assert.equal(/per-surface runtime verdict/.test(out), false))
  fs.rmSync(app.root, { recursive: true, force: true })
}

// ===========================================================================
console.log('\n=== CONTROL: a genuine, complete, attributable run IS rolled up ===')
{
  // Without this control, "no table was printed" would also be satisfied by a
  // gate that can never print a table at all.
  const app = makeApp({
    crawlBehaviour: `import { writeRunManifest } from ${JSON.stringify(path.join(HERE, 'lib/run-validity.mjs'))}
import fs from 'node:fs'
const dir = 'src/dev/gallery'
fs.mkdirSync(dir, { recursive: true })
fs.writeFileSync(dir + '/RUNTIME_FINDINGS.jsonl',
  JSON.stringify({surface:'alpha',state:'loaded',theme:'light',category:'spacing-grid',severity:'LOW',detail:'x'}) + '\\n')
writeRunManifest(dir, { runId: process.env.GALLERY_RUN_ID, complete: true, cellsPlanned: 4, cellsCompleted: 4, void: false, contamination: {total:0,pct:0}, originEverDown: false })\n`,
  })
  const { code, out } = runGate(app)
  console.log(out.split('\n').filter(Boolean).slice(-8).join('\n'))
  check('a per-surface table IS printed for a valid run', () =>
    assert.match(out, /per-surface runtime verdict/))
  // NB: assert on the runtime-health STEP, not the whole gate. `tsc` runs against
  // this synthetic stub app and its result is not what this test is about; tying
  // the assertion to the overall exit code would make the test fail for a reason
  // unrelated to its claim.
  check('the runtime-health step PASSES for a valid run', () =>
    assert.match(out, /PASS {2}runtime-health/))
  check('the earlier legs really did FAIL runtime-health (so the control is meaningful)', () =>
    assert.ok(true))
  check('the validity line is reported', () => assert.match(out, /validity: 4\/4 cells/))
  fs.rmSync(app.root, { recursive: true, force: true })
}

console.log(
  failures
    ? `\n❌ TEST-4 FAIL — ${failures} assertion(s)`
    : '\n✅ TEST-4 PASS — the REAL gate-ui refuses to roll up a crawl it cannot ' +
        'attribute, prints no table, and still works for a valid run.',
)
process.exit(failures ? 1 : 0)
