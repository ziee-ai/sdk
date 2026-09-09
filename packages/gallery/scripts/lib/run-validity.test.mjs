/**
 * TEST — run validity + the run manifest (TEST-4 acceptance / TEST-8..11 / 19 / 20).
 *
 * Run: node --test scripts/lib/run-validity.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import {
  CONSOLE_TRANSPORT_MIRROR,
  DYN_IMPORT_FAILURE,
  TRANSPORT_DEAD,
  assessRun,
  clearRunArtifacts,
  contaminationOf,
  readRunManifest,
  verifyRunManifest,
  writeRunManifest,
} from './run-validity.mjs'

const tmp = () => fs.mkdtempSync(path.join(os.tmpdir(), 'runval-'))
const F = (over = {}) => ({
  surface: 's',
  state: 'loaded',
  theme: 'light',
  category: 'console-error',
  severity: 'HIGH',
  detail: 'x',
  ...over,
})

// ---------------------------------------------------------------------------
// TEST-4 [acceptance · INV-4] — a run that did not complete CANNOT be rolled up.
// ---------------------------------------------------------------------------
test('TEST-4 [acceptance INV-4] an incomplete run cannot inherit a previous run\'s data', () => {
  const dir = tmp()

  // Stage the exact hazard: a PREVIOUS run's all-clean artifacts sitting on disk.
  fs.writeFileSync(
    path.join(dir, 'RUNTIME_FINDINGS.jsonl'),
    `${JSON.stringify(F({ severity: 'LOW', category: 'spacing-grid' }))}\n`,
  )
  writeRunManifest(dir, {
    runId: 'PREVIOUS-RUN',
    complete: true,
    cellsPlanned: 682,
    cellsCompleted: 682,
    void: false,
  })
  assert.ok(readRunManifest(dir), 'the previous run\'s manifest is on disk (the hazard)')

  // A new gate run clears first, then its crawl is killed at 575/682 — so it
  // writes nothing at all.
  clearRunArtifacts(dir)
  assert.equal(readRunManifest(dir), null, 'nothing survives the clear')
  assert.equal(
    fs.existsSync(path.join(dir, 'RUNTIME_FINDINGS.jsonl')),
    false,
    'the previous findings file is gone — there is nothing to inherit',
  )

  const v = verifyRunManifest(readRunManifest(dir), 'THIS-RUN')
  assert.equal(v.ok, false, 'the roll-up must be refused')
  assert.match(v.reason, /did not complete/)
  assert.match(v.reason, /PREVIOUS run/i, 'the reason must name the actual hazard')
})

test('TEST-19 the manifest validation matrix — each refusal names its OWN cause', () => {
  const base = {
    runId: 'R1',
    complete: true,
    cellsPlanned: 100,
    cellsCompleted: 100,
    void: false,
  }
  assert.equal(verifyRunManifest(base, 'R1').ok, true, 'all-good accepts')

  const cases = [
    [null, /did not complete/, 'missing manifest'],
    [{ ...base, complete: false }, /complete=false/, 'complete:false'],
    [{ ...base, runId: 'OTHER' }, /run id mismatch/i, 'runId mismatch'],
    [
      { ...base, cellsPlanned: 682, cellsCompleted: 575 },
      /only 575 of 682/,
      'truncated crawl',
    ],
    [
      { ...base, void: true, voidReasons: ['origin died'] },
      /VOID.*origin died/,
      'self-declared void',
    ],
  ]
  for (const [m, re, label] of cases) {
    const v = verifyRunManifest(m, 'R1')
    assert.equal(v.ok, false, `${label} must be refused`)
    assert.match(v.reason, re, `${label}: the reason must be specific`)
  }
})

test('TEST-20 the manifest is written atomically and records the run shape', () => {
  const dir = tmp()
  writeRunManifest(dir, {
    runId: 'R9',
    cellsPlanned: 682,
    cellsCompleted: 682,
    complete: true,
    void: false,
  })
  const m = readRunManifest(dir)
  assert.equal(m.runId, 'R9')
  assert.equal(m.cellsPlanned, 682)
  assert.equal(m.cellsCompleted, 682)
  // No temp file left behind — a half-written manifest must never be observable.
  const leftovers = fs.readdirSync(dir).filter(f => f.includes('.tmp'))
  assert.deepEqual(leftovers, [], 'write-temp-then-rename leaves no partial file')
})

test('TEST-20b a corrupt manifest reads as absent, not as a crash', () => {
  const dir = tmp()
  fs.writeFileSync(path.join(dir, 'RUNTIME_RUN.json'), '{{{ not json')
  assert.equal(readRunManifest(dir), null)
  assert.equal(verifyRunManifest(readRunManifest(dir), 'R1').ok, false)
})

// ---------------------------------------------------------------------------
// TEST-9/10/11 — transport classification, BOTH directions.
// ---------------------------------------------------------------------------
test('TEST-9 the console mirror pattern matches a transport error, not an HTTP status', () => {
  assert.ok(CONSOLE_TRANSPORT_MIRROR.test('Failed to load resource: net::ERR_ABORTED'))
  assert.ok(
    CONSOLE_TRANSPORT_MIRROR.test('Failed to load resource: net::ERR_NETWORK_CHANGED'),
  )
  // The HTTP-status mirror is a DIFFERENT, already-handled class — the two must
  // not be conflated, or the gallery's deliberate 500-injection would be muted
  // by the transport arm instead of by its own documented rule.
  assert.equal(
    CONSOLE_TRANSPORT_MIRROR.test(
      'Failed to load resource: the server responded with a status of 500',
    ),
    false,
  )
  assert.equal(CONSOLE_TRANSPORT_MIRROR.test('TypeError: x is not a function'), false)
})

test('TEST-9b every transport error observed in the field is recognised', () => {
  // ERR_CONNECTION_RESET  — origin killed mid-crawl (probe F)
  // ERR_ABORTED           — Vite HMR full reload mid-crawl (flake run02, 538 of them)
  // ERR_NETWORK_CHANGED   — foreign-worktree port collision (run-key.mjs)
  for (const e of [
    'net::ERR_CONNECTION_RESET',
    'net::ERR_CONNECTION_REFUSED',
    'net::ERR_ABORTED',
    'net::ERR_NETWORK_CHANGED',
    'net::ERR_EMPTY_RESPONSE',
  ])
    assert.ok(TRANSPORT_DEAD.test(`GET /x — ${e}`), e)
  // An HTTP status is NOT a transport failure.
  assert.equal(TRANSPORT_DEAD.test('GET /api/x — 500'), false)
})

test('TEST-10 the dynamic-import crash pattern is recognised', () => {
  assert.ok(
    DYN_IMPORT_FAILURE.test(
      '[AppErrorBoundary] Failed to fetch dynamically imported module: http://x/src/a.js',
    ),
  )
  assert.equal(
    DYN_IMPORT_FAILURE.test('[AppErrorBoundary] TypeError: cannot read x'),
    false,
    'an ordinary ErrorBoundary crash must never match',
  )
})

test('TEST-11 contaminationOf counts all three faces and the percentage', () => {
  const findings = [
    F({ category: 'request-failed', detail: 'GET /@fs/a.js — net::ERR_ABORTED' }),
    F({ category: 'request-failed', detail: 'GET /@fs/b.js — net::ERR_ABORTED' }),
    F({ category: 'console-error', detail: 'Failed to load resource: net::ERR_ABORTED' }),
    F({
      category: 'crash',
      detail: '[AppErrorBoundary] Failed to fetch dynamically imported module: http://x/a.js',
    }),
    F({ category: 'console-error', detail: 'TypeError: real product bug' }),
  ]
  const c = contaminationOf(findings)
  assert.equal(c.requestFailed, 2)
  assert.equal(c.consoleMirror, 1)
  assert.equal(c.dynImportCrash, 1)
  assert.equal(c.total, 4)
  assert.equal(c.pct, 80)
})

test('TEST-11b a clean finding set reports 0 / 0%', () => {
  const c = contaminationOf([F({ detail: 'TypeError: real bug' })])
  assert.equal(c.total, 0)
  assert.equal(c.pct, 0)
  assert.deepEqual(contaminationOf([]), {
    requestFailed: 0,
    consoleMirror: 0,
    dynImportCrash: 0,
    total: 0,
    pct: 0,
  })
})

// ---------------------------------------------------------------------------
// TEST-1b/2b — the run-validity verdict (the D1 replacement).
// ---------------------------------------------------------------------------
test('TEST-1b a run whose origin went down is VOID, and says why', () => {
  const v = assessRun({
    findings: [F()],
    origin: { everDown: true, downAt: '2026-08-08T18:00:00Z', checks: 12 },
    cellsPlanned: 682,
    cellsCompleted: 682,
  })
  assert.equal(v.void, true)
  assert.match(v.reasons[0], /origin was UNREACHABLE/)
  assert.match(v.reasons[0], /2026-08-08T18:00:00Z/, 'name WHEN it went down')
  assert.match(v.reasons[0], /describe the harness, not the product/)
})

test('TEST-1c a run dominated by transport artifacts is VOID even if the origin sampled OK', () => {
  // The field case: 10,430 of 10,925 findings were this artifact (95.5%). The
  // origin can flap between 5s samples, so the finding population is the second,
  // independent detector.
  const findings = [
    ...Array.from({ length: 95 }, (_, i) =>
      F({ category: 'request-failed', detail: `GET /@fs/${i}.js — net::ERR_ABORTED` }),
    ),
    ...Array.from({ length: 5 }, () => F({ detail: 'TypeError: real bug' })),
  ]
  const v = assessRun({
    findings,
    origin: { everDown: false, checks: 20 },
    cellsPlanned: 682,
    cellsCompleted: 682,
  })
  assert.equal(v.void, true)
  assert.match(v.reasons[0], /95%|95\b/)
  assert.match(v.reasons[0], /past BOTH the 50-artifact floor and the 25% ratio/)
})

test('TEST-1c2 [calibration] every run this change has data for gets the right verdict', () => {
  // The bar must be robust at BOTH ends. A ratio alone false-FAILED a healthy run
  // (routine dev-asset aborts are already muted as harness noise, and origin/main
  // itself carries 36 of them); an absolute floor alone would false-PASS a huge
  // contaminated run. Both are required together.
  const mk = (artifacts, others) => [
    ...Array.from({ length: artifacts }, (_, i) =>
      F({ category: 'request-failed', detail: `GET /@fs/${i}.js — net::ERR_ABORTED` }),
    ),
    ...Array.from({ length: others }, () =>
      F({ severity: 'LOW', category: 'spacing-grid', detail: 'padding 7px' }),
    ),
  ]
  const verdict = (a, o) =>
    assessRun({
      findings: mk(a, o),
      origin: { everDown: false, checks: 50 },
      cellsPlanned: 1,
      cellsCompleted: 1,
    }).void

  // MEASURED populations, by name:
  assert.equal(verdict(36, 572), false, 'origin/main baseline (36/608, 5.9%) — VALID')
  assert.equal(verdict(0, 531), false, 'a clean full crawl on this box (0/531) — VALID')
  assert.equal(verdict(538, 487), true, 'the HMR-disturbed crawl (538/1025, 52.5%) — VOID')
  assert.equal(verdict(10430, 495), true, 'the reported field case (95.5%) — VOID')
  // Boundaries the auditor named:
  assert.equal(verdict(2, 13), false, 'a SHORT crawl must not void on 2 artifacts (13%)')
  assert.equal(verdict(49, 51), false, 'below the absolute floor, however high the ratio')
  assert.equal(verdict(600, 9400), false, 'below the ratio, however high the count')
  assert.equal(verdict(60, 100), true, 'past both — void')
})

test('TEST-1d a HEALTHY run is NOT void — the gate must not cry wolf', () => {
  // Measured shape of a real clean run on this box: 531 findings, ZERO transport
  // artifacts. If this went void, every run would be void and the gate would be
  // useless in the opposite direction.
  const findings = Array.from({ length: 531 }, () => F({ detail: 'contrast 3.9:1' }))
  const v = assessRun({
    findings,
    origin: { everDown: false, checks: 140 },
    cellsPlanned: 682,
    cellsCompleted: 682,
  })
  assert.equal(v.void, false, 'a clean run must pass')
  assert.deepEqual(v.reasons, [])
  assert.equal(v.contamination.total, 0)
})

test('TEST-1e a truncated crawl is VOID', () => {
  const v = assessRun({
    findings: [F()],
    origin: { everDown: false, checks: 90 },
    cellsPlanned: 682,
    cellsCompleted: 575, // the exact observed truncation
  })
  assert.equal(v.void, true)
  assert.match(v.reasons[0], /575 of 682/)
})

test('a few transport artifacts below the bar do NOT void an otherwise good run', () => {
  const findings = [
    ...Array.from({ length: 3 }, () =>
      F({ category: 'request-failed', detail: 'GET /@fs/a.js — net::ERR_ABORTED' }),
    ),
    ...Array.from({ length: 100 }, () => F({ detail: 'contrast 3.9:1' })),
  ]
  const v = assessRun({
    findings,
    origin: { everDown: false, checks: 50 },
    cellsPlanned: 10,
    cellsCompleted: 10,
  })
  assert.equal(v.void, false)
  assert.ok(v.contamination.total === 3 && v.contamination.pct < 10)
})

// ---------------------------------------------------------------------------
// TEST-7 — the ORIGIN WATCHER. Previously untested and structurally vacuous at
// the edges; both are now pinned. A real HTTP fixture is used, not a stub — the
// watcher's whole job is to observe a real server, so a stubbed probe would
// prove nothing about it.
// ---------------------------------------------------------------------------
import http from 'node:http'
import { originAlive, watchOrigin } from './run-validity.mjs'

const listen = handler =>
  new Promise(res => {
    const s = http.createServer(handler)
    s.listen(0, '127.0.0.1', () => res(s))
  })

test('TEST-7 originAlive is true for a live origin, false for a dead one', async () => {
  const s = await listen((_q, r) => {
    r.writeHead(200)
    r.end('ok')
  })
  const url = `http://127.0.0.1:${s.address().port}/`
  assert.equal(await originAlive(url), true)
  await new Promise(r => s.close(r))
  assert.equal(await originAlive(url, 1500), false, 'a closed origin is not alive')
})

test('TEST-7b a SINGLE slow/failed probe does NOT declare the origin down', async () => {
  // The false-FAIL guard. A first version latched everDown on one failed sample,
  // so a single GC pause on a busy dev server discarded a 12-minute crawl. If
  // `requiredFailures` were reverted to 1 this test goes red.
  let fail = true
  const s = await listen((_q, r) => {
    if (fail) {
      fail = false // exactly ONE failure, then healthy forever
      r.destroy()
      return
    }
    r.writeHead(200)
    r.end('ok')
  })
  const w = watchOrigin(`http://127.0.0.1:${s.address().port}/`, {
    intervalMs: 20,
    requiredFailures: 3,
  })
  await new Promise(r => setTimeout(r, 400))
  const st = w.stop()
  await new Promise(r => s.close(r))
  assert.equal(st.everDown, false, 'one transient failure must not void a run')
  assert.ok(st.checks >= 3, `the watcher must actually have sampled (got ${st.checks})`)
  assert.equal(st.maxConsecutiveFailures, 1, 'it saw the blip, it just did not latch')
})

test('TEST-7c a genuinely dead origin IS declared down after confirmation', async () => {
  const s = await listen((_q, r) => {
    r.writeHead(200)
    r.end('ok')
  })
  const url = `http://127.0.0.1:${s.address().port}/`
  const w = watchOrigin(url, { intervalMs: 20, requiredFailures: 3 })
  await new Promise(r => setTimeout(r, 120))
  await new Promise(r => s.close(r)) // origin dies mid-"crawl"
  await new Promise(r => setTimeout(r, 600))
  const st = w.stop()
  assert.equal(st.everDown, true, 'a server that stays dead must be detected')
  assert.ok(st.downAt, 'and the moment recorded')
  assert.ok(st.maxConsecutiveFailures >= 3)
})

test('TEST-7d the watcher samples IMMEDIATELY — a short run never reports checks:0', async () => {
  // Without an immediate first sample the first probe fired at intervalMs, so a
  // short crawl finished with checks:0 and was reported "origin alive" on no
  // evidence at all.
  const s = await listen((_q, r) => {
    r.writeHead(200)
    r.end('ok')
  })
  const w = watchOrigin(`http://127.0.0.1:${s.address().port}/`, { intervalMs: 60_000 })
  await new Promise(r => setTimeout(r, 150))
  const st = w.stop()
  await new Promise(r => s.close(r))
  assert.ok(st.checks >= 1, 'the first sample must not wait for the full interval')
})

test('TEST-7e stop() is final — a probe landing afterwards cannot mutate the snapshot', async () => {
  const s = await listen((_q, r) => {
    r.writeHead(200)
    r.end('ok')
  })
  const w = watchOrigin(`http://127.0.0.1:${s.address().port}/`, { intervalMs: 10 })
  const st = w.stop() // stop immediately, while the first probe is in flight
  await new Promise(r => setTimeout(r, 200))
  await new Promise(r => s.close(r))
  assert.equal(st.everDown, false)
  assert.equal(typeof st.checks, 'number')
})
