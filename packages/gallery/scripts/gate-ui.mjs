/**
 * EVALUATOR GATE — the single UI-surface exit condition.
 *
 * A UI surface is DONE only if ALL of the following hold. This script runs each
 * check, prints a per-surface PASS/FAIL table, and exits non-zero on any failure.
 *
 *   1. tsc      — `tsc --noEmit` is clean (types compile).
 *   2. lint     — biome guardrails + hardcoded-color lint are clean.
 *   3. runtime  — the runtime-health pass reports ZERO HIGH findings for the
 *                 surface (no console error / pageerror / failed request / crash /
 *                 WCAG-AA contrast failure).
 *   4. visual   — the deterministic visual layer passes: Layer A layout invariants
 *                 + axe a11y (always), and Layer B pixel regression (toHave
 *                 Screenshot) when VISUAL_SNAPSHOTS=1 with blessed baselines.
 *
 * (The 5th DONE criterion — "no HIGH finding from the Opus/Sonnet visual critic
 * pass" — is a human/vision-model review step documented in CLAUDE.md; it is not
 * mechanizable here, so the gate asserts the four automatable conditions and the
 * critic sign-off is recorded out of band.)
 *
 * Flags: --skip-visual (skip the visual layer) · --skip-extra (skip every
 * app-declared extra stage; --skip-coverage is an alias kept from the deleted
 * desktop fork, which had exactly one extra stage) · --no-wait (do not queue on
 * the host lock).
 *
 * Usage:
 *   npm run gate:ui                 # full gate (Layer B skipped unless snapshots)
 *   VISUAL_SNAPSHOTS=1 npm run gate:ui   # include pixel regression
 *   npm run gate:ui -- --skip-visual     # runtime + tsc + lint only (fast)
 */
import { spawn, spawnSync } from 'node:child_process'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { resolveGalleryConfig, resolveVisualTier } from './lib/gallery-config.mjs'
import {
  resolveGalleryPort,
  pickBindablePort,
  resolveWorktreeRoot,
  fetchSentinelRoot,
  serverIsThisWorktree,
} from './lib/run-key.mjs'
import { acquire, TOKEN_ENV } from './lib/host-lock.mjs'
import {
  clearRunArtifacts,
  newRunId,
  readRunManifest,
  verifyRunManifest,
} from './lib/run-validity.mjs'

const __dirname = path.dirname(fileURLToPath(import.meta.url))
// Config-driven anchors (was UI_DIR/GALLERY_DIR/port/visual hardcodes). The app
// runs this with its `ui/` as cwd, so `gallery.config.json` + relative paths
// resolve against it.
const CFG = resolveGalleryConfig()
// These scripts are SHARED by several app workspaces and are addressed by a long
// relative path (`node ../../../sdk/packages/gallery/scripts/…`), so running them
// from the wrong cwd is easy. Without a config file every anchor silently falls
// back to the WEB defaults — including `portWhich`, which would point a desktop
// run at the web gallery's port and let it be accepted as "ours". A bad PORT
// already fails loudly two lines away; a missing config must too.
if (!fs.existsSync(CFG.__configPath)) {
  console.error(
    `gate-ui: refusing to run — no gallery.config.json at ${CFG.__configPath}.\n` +
      `  These scripts read their anchors (galleryDir, portWhich, galleryUrl, …) from the\n` +
      `  APP's config, so cwd must be the app's ui/ directory — the one whose\n` +
      `  gallery.config.json you mean. Running elsewhere would silently use another\n` +
      `  app's defaults (including portWhich, i.e. another gallery's port).`,
  )
  process.exit(2)
}
const UI_DIR = CFG.__cwd
const GALLERY_DIR = path.resolve(UI_DIR, CFG.galleryDir)
// Overridable ONLY so the end-to-end test can substitute a stub crawl and drive
// the real gate. Unset in every normal run.
const RUNTIME_HEALTH =
  process.env.GATE_UI_RUNTIME_HEALTH || path.join(__dirname, 'runtime-health.mjs')
// Key-derived, bind-checked port BASE (audit §7: no fixed 1420). The key only
// seeds the search start; the actual PORT is decided in main() — reuse the base
// ONLY if the server already there proves it's THIS worktree (sentinel), else
// boot our own on a fresh bindable port. Precedence: GALLERY_PORT > cfg.port >
// key-derived.
const OWN_ROOT = resolveWorktreeRoot(UI_DIR)
const PORT_BASE = resolveGalleryPort({
  env: process.env.GALLERY_PORT,
  cfgPort: CFG.port ?? null,
  which: CFG.portWhich || 'webGallery',
  cwd: UI_DIR,
})
let PORT = PORT_BASE // finalized in main()
const SKIP_VISUAL = process.argv.includes('--skip-visual')
// `--skip-coverage` is kept as an alias: it is what the (now-deleted) desktop
// fork of this script accepted, and desktop's extra step is the coverage check.
const SKIP_EXTRA =
  process.argv.includes('--skip-extra') || process.argv.includes('--skip-coverage')

const results = [] // { name, ok, detail }
const step = (name, ok, detail = '') => {
  results.push({ name, ok, detail })
  console.log(`${ok ? '✅' : '❌'} ${name}${detail ? ` — ${detail}` : ''}`)
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, {
    cwd: UI_DIR,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
    ...opts,
  })
  return { code: r.status ?? 1, out: (r.stdout || '') + (r.stderr || '') }
}

// HTTP health check against the gallery entry (matches how the pass reaches it —
// a TCP probe can miss a vite server bound only to IPv6 localhost).
const galleryUp = async port => {
  try {
    const ac = new AbortController()
    const t = setTimeout(() => ac.abort(), 1500)
    const r = await fetch(`http://localhost:${port}${CFG.galleryUrl}`, {
      signal: ac.signal,
    })
    clearTimeout(t)
    return r.ok
  } catch {
    return false
  }
}

async function waitForPort(port, timeoutMs) {
  const start = Date.now()
  while (Date.now() - start < timeoutMs) {
    if (await galleryUp(port)) return true
    await new Promise(r => setTimeout(r, 500))
  }
  return false
}

let lockToken = null

/**
 * Acquire the host lock for the shared-resource part of the gate.
 *
 * The wait budget is deliberately generous (45 min default, ~3 full crawls):
 * with a short budget the THIRD concurrent run on this machine hard-fails with
 * exit 2, which in an automated pipeline is indistinguishable from a genuine
 * gate failure — converting a throughput problem into a false FAIL. Waiting is
 * the correct behaviour; timing out is the exception.
 */
async function withHostLockAcquire() {
  const lock = await acquire({
    owner: OWN_ROOT,
    wait: !process.argv.includes('--no-wait'),
    waitMs: Number(process.env.GATE_UI_LOCK_WAIT_MS) || 45 * 60 * 1000,
    log: m => console.log(m),
  })
  // finish() calls process.exit(), which skips `finally` — so also release on exit.
  process.on('exit', () => lock.release())
  return lock
}

async function main() {
  console.log('=== UI evaluator gate ===\n')

  // 1. tsc -------------------------------------------------------------------
  console.log('• typecheck (tsc --noEmit) …')
  const tsc = run('npx', ['tsc', '--noEmit'])
  step(
    'tsc',
    tsc.code === 0,
    tsc.code === 0 ? 'clean' : `${(tsc.out.match(/error TS/g) || []).length} errors`,
  )

  // 2. lint ------------------------------------------------------------------
  console.log('• lint (guardrails + colors) …')
  const lintRuns = CFG.lintCmds.map(([cmd, args]) => run(cmd, args))
  const lintOk = lintRuns.every(r => r.code === 0)
  step('lint', lintOk, lintOk ? 'clean' : 'violations (see output above)')
  for (const r of lintRuns) if (r.code !== 0) console.log(r.out.slice(-1500))

  // 3. + 4. runtime + visual need the gallery Vite server. Boot it (or reuse
  // ONLY our own). NO-FOREIGN-REUSE (audit §2/§7): reuse a server already on the
  // base port ONLY if its /__worktree sentinel proves it belongs to THIS
  // worktree — otherwise a run silently tests a sibling worktree's source tree
  // (the fixed-1420 false-pass). If the base holds a FOREIGN server (or nothing),
  // pick a fresh bindable port and boot our own.
  // 3. + 4. need the gallery server, and BOTH contend with a concurrent run on
  // this machine. Take the host lock HERE rather than around the whole gate:
  // `tsc` and the lint passes touch nothing shared, and holding the lock through
  // them needlessly extends every other run's wait. With a bounded wait budget
  // that difference decides whether a third concurrent run hard-fails.
  const lock = await withHostLockAcquire()
  lockToken = lock.token ?? null

  let vite = null
  const sentinelRoot = await fetchSentinelRoot(PORT_BASE)
  const isOurs = serverIsThisWorktree(sentinelRoot, OWN_ROOT) && (await galleryUp(PORT_BASE))
  if (isOurs) {
    PORT = PORT_BASE
    console.log(`• reusing THIS worktree's gallery dev server already on :${PORT}`)
  } else {
    if (sentinelRoot && sentinelRoot !== OWN_ROOT) {
      console.log(
        `• port :${PORT_BASE} holds a FOREIGN worktree's server (${sentinelRoot}); NOT reusing — booting our own on a fresh port`,
      )
    }
    PORT = await pickBindablePort(PORT_BASE)
    console.log(`• booting gallery dev server on :${PORT} …`)
    const [devCmd, ...devArgs] = CFG.devCmd
    vite = spawn(
      devCmd,
      [...devArgs, '--', '--port', String(PORT), '--strictPort'],
      { cwd: UI_DIR, stdio: 'ignore', detached: false },
    )
    const up = await waitForPort(PORT, 120_000)
    if (!up) {
      step('gallery-server', false, `did not come up on :${PORT}`)
      finish()
      return
    }
  }

  let runUsable = true
  try {
    // 3. runtime-health --------------------------------------------------------
    console.log('• runtime-health pass …')
    // Delete the PREVIOUS run's findings + manifest FIRST. This is what makes
    // inheriting stale data structurally impossible rather than merely detected:
    // a crawl that is killed mid-flight now leaves an EMPTY directory, so there
    // is nothing for the roll-up to read. Previously a killed run at 575/682
    // cells produced a confident `103/106 PASS` table computed entirely from an
    // earlier run's file, distinguishable only by an unchanged mtime.
    const runId = newRunId()
    clearRunArtifacts(GALLERY_DIR)
    const rt = run('node', [RUNTIME_HEALTH, '--report-only'], {
      env: {
        ...process.env,
        GALLERY_PORT: String(PORT),
        GALLERY_RUN_ID: runId,
        // The child must NOT contend for the host lock we already hold.
        [TOKEN_ENV]: lockToken || 'inherited',
      },
    })
    console.log(rt.out.split('\n').slice(-10).join('\n'))

    // The run must PROVE it completed and is attributable to us before any of
    // its output may be turned into a verdict.
    const manifest = readRunManifest(GALLERY_DIR)
    const verdictOk = verifyRunManifest(manifest, runId)
    if (!verdictOk.ok) {
      step('runtime-health', false, `run not usable — ${verdictOk.reason}`)
      console.log(
        `\n   The runtime crawl did not produce a usable result, so NO per-surface\n` +
          `   verdict is available. This is deliberately NOT reported as a pass or a\n` +
          `   per-surface table: a table printed here would be a previous run's data.\n` +
          `   Child exit code: ${rt.code}.`,
      )
      // Skip the visual layer too — it audits the same gallery this run could
      // not measure. Fall THROUGH to the finally below rather than exiting here:
      // finish() calls process.exit(), which would skip the finally and orphan
      // the Vite server we spawned (CODING_GUIDELINES §5).
      runUsable = false
    }

    let surfaceVerdicts = []
    if (runUsable) {
      surfaceVerdicts = readRuntimeSurfaceVerdicts()
      const runtimeFail = surfaceVerdicts.filter(s => !s.ok)
      const c = manifest.contamination || {}
      console.log(
        `   validity: ${manifest.cellsCompleted}/${manifest.cellsPlanned} cells · origin ` +
          `${manifest.originEverDown ? 'WENT DOWN' : 'alive'} · transport artifacts ` +
          `${c.total ?? 0} (${c.pct ?? 0}% of findings)`,
      )
      step(
        'runtime-health',
        rt.code === 0 && runtimeFail.length === 0,
        runtimeFail.length
          ? `${runtimeFail.length} surface(s) with HIGH findings`
          : `${surfaceVerdicts.length} surfaces clean`,
      )
    }

    // 4. visual layer (Layer A always; Layer B when VISUAL_SNAPSHOTS) ----------
    const tier = resolveVisualTier(CFG, { snapshots: !!process.env.VISUAL_SNAPSHOTS })
    // (chat) Skipped entirely when the crawl was unusable: the visual layer
    // audits the same gallery this run could not measure, so a "visual PASS"
    // beside an unusable runtime result is exactly the kind of confident-looking
    // output this change exists to prevent. A DECLARED-absent visual tier
    // (`visualConfig: null`) is handled by the `!tier.enabled` branch below via
    // resolveVisualTier() — no separate `!CFG.visualConfig` check needed.
    if (!runUsable) {
      step('visual', false, 'skipped — the runtime crawl was not usable')
    } else if (SKIP_VISUAL) {
      step('visual', true, 'skipped (--skip-visual)')
    } else if (!tier.enabled) {
      // A DECLARED-absent visual tier is a skip, not a failure. Interpolating a
      // null `visualConfig` into the argv used to hand playwright `-c null`,
      // which died on `<uiRoot>/null does not exist` and failed the gate for an
      // app whose config had said, in the config's own vocabulary, that it has
      // no pixel tier yet.
      step('visual', true, `skipped (${tier.reason})`)
    } else {
      console.log('• visual layer (layout + axe + regression) …')
      const vis = run(
        'npx',
        ['playwright', 'test', '-c', tier.config, ...tier.specs],
        { env: { ...process.env, GALLERY_PORT: String(PORT) } },
      )
      const passed = (vis.out.match(/(\d+) passed/) || [])[1]
      const failed = (vis.out.match(/(\d+) failed/) || [])[1]
      step(
        'visual',
        vis.code === 0,
        vis.code === 0
          ? `${passed ?? '?'} passed`
          : `${failed ?? '?'} failed`,
      )
      if (vis.code !== 0) console.log(vis.out.split('\n').slice(-25).join('\n'))
    }

    // 5. app-declared extra steps (config-driven, same shape as lintCmds) ------
    for (const [label, cmd, args] of CFG.gateExtraCmds ?? []) {
      if (!runUsable) {
        // FAIL, matching what `visual` does under the SAME condition twenty lines
        // above. finish() prints only `PASS <name>` in the summary block, so a
        // softened pass here reads as `PASS coverage` for a step that never ran —
        // a verdict for something that did not execute. The gate is already red
        // on runtime-health, so failing costs nothing and claims nothing.
        step(label, false, 'not run — the runtime crawl was not usable')
        continue
      }
      if (SKIP_EXTRA) {
        step(label, true, 'skipped (--skip-extra)')
        continue
      }
      console.log(`• ${label} …`)
      const r = run(cmd, args)
      step(label, r.code === 0, r.code === 0 ? 'ok' : 'failed (see output above)')
      if (r.code !== 0) console.log(r.out.slice(-1500))
    }

    printSurfaceTable(surfaceVerdicts)
  } finally {
    if (vite) {
      try {
        vite.kill('SIGTERM')
      } catch {
        /* ignore */
      }
    }
    lock.release()
  }

  finish()
}

/** Roll the runtime JSONL up into a per-surface PASS/FAIL (fail iff any HIGH). */
function readRuntimeSurfaceVerdicts() {
  const p = path.join(GALLERY_DIR, 'RUNTIME_FINDINGS.jsonl')
  if (!fs.existsSync(p)) return []
  const surfaces = {}
  for (const line of fs.readFileSync(p, 'utf8').split('\n').filter(Boolean)) {
    let f
    try {
      f = JSON.parse(line)
    } catch {
      continue
    }
    const s = (surfaces[f.surface] ??= { high: 0, medium: 0, low: 0, baselined: 0 })
    // A documented-baselined (runtime-baseline.js) OR documented-harness-noise
    // (`f.harness` — dev-server/mock-cassette artifact, e.g. a `@fs` node_modules
    // font 403) HIGH does not fail a surface. This mirrors runtime-health.mjs's
    // gating formula (HIGH − baselined − harness); without the `f.harness` term,
    // gate:ui would fail on harness noise that runtime-health itself treats as
    // non-gating (surfaces only through symlinked node_modules, e.g. a worktree).
    if (f.baselined || f.harness) s.baselined++
    else s[f.severity.toLowerCase()]++
  }
  return Object.entries(surfaces)
    .map(([surface, c]) => ({ surface, ...c, ok: c.high === 0 }))
    .sort((a, b) => b.high - a.high || a.surface.localeCompare(b.surface))
}

function printSurfaceTable(verdicts) {
  if (!verdicts.length) return
  const fails = verdicts.filter(v => !v.ok)
  console.log(
    `\n--- per-surface runtime verdict: ${verdicts.length - fails.length}/${verdicts.length} PASS ---`,
  )
  if (fails.length) {
    for (const v of fails)
      console.log(`   ❌ ${v.surface}  (HIGH ${v.high}, MEDIUM ${v.medium})`)
  } else {
    console.log('   ✅ all surfaces runtime-clean')
  }
}

function finish() {
  const failed = results.filter(r => !r.ok)
  console.log('\n=== gate summary ===')
  for (const r of results) console.log(`  ${r.ok ? 'PASS' : 'FAIL'}  ${r.name}`)
  if (failed.length) {
    console.log(`\n❌ GATE FAILED — ${failed.map(f => f.name).join(', ')}`)
    process.exit(1)
  }
  console.log('\n✅ GATE PASSED — every UI DONE criterion met')
  process.exit(0)
}

// The WHOLE gate holds the host lock — not just the crawl. Two concurrent
// gate:ui runs on one machine take each other's dev server away (a port race, a
// `finally` that kills a server the other's browser is still pulling modules
// from), which is the proven cause of the mass transport contamination. Silence
// on contention — the observed behaviour — is not representable any more: the
// waiter announces the holding worktree + pid.
main().catch(e => {
  console.error(e.message || e)
  process.exit(2)
})
