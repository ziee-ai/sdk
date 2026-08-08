/**
 * RUN VALIDITY — is this crawl's output allowed to be believed?
 *
 * Two independent failure modes, both observed in the field, both of which
 * previously produced a CONFIDENT-LOOKING verdict over data that meant nothing:
 *
 * 1. **The origin went away mid-crawl.** Proven mechanism (probe F, DRIFT-1.2):
 *    when the gallery's Vite server dies or is stolen while a page has a lazy ES
 *    module in flight, Chromium reports the failure THREE ways — `requestfailed`,
 *    a `console.error` mirror (`Failed to load resource: net::ERR_…`), and a
 *    rejected dynamic import that trips the app's ErrorBoundary as a `crash`. The
 *    classifier muted only the first, so a single dead server turned into
 *    thousands of gating HIGHs and a handful of fabricated "product crashes".
 *    One run: 10,925 findings, 10,430 of them this artifact (95.5%).
 *    `runtime-health`'s own port guard already fails loudly for a PORTLESS url
 *    that could never work; this covers the harder case of an origin that worked
 *    when the run started and stopped working during it.
 *
 * 2. **The crawl did not finish.** A killed run wrote no findings file, so
 *    `gate-ui` rolled up the PREVIOUS run's and printed `103/106 PASS` over it —
 *    detectable only by a truncated cell count and an unchanged mtime. The run
 *    manifest below makes a run's output attributable to that run, so inheriting
 *    stale data is structurally impossible rather than merely noticed.
 *
 * The rule in both cases is the same, and it is the one INV-4 states: **a run
 * that could not observe the product must fail loudly, not present noise as
 * verdicts.**
 */
import fs from 'node:fs'
import path from 'node:path'
import crypto from 'node:crypto'

/**
 * Transport errors that mean "the request never got an answer", as distinct from
 * an HTTP status (which means the origin WAS reachable and DID answer).
 *
 * Bare `ERR_ABORTED` is deliberately included and is the MOST COMMON of these in
 * practice: it is what a Vite HMR full-reload produces when a source file in the
 * gallery's module graph changes mid-crawl, aborting every ESM import in flight.
 * One measured run produced 538 of them (vs 0 in the immediately preceding run on
 * the same server) after a generated file was rewritten during the crawl. Omitting
 * it — as a first draft of this regex did — makes the validity gate blind to the
 * trigger most likely to fire while someone is actually working.
 */
export const TRANSPORT_DEAD =
  /net::ERR_(ABORTED|FAILED|CONNECTION_(REFUSED|RESET|CLOSED|ABORTED|TIMED_OUT|FAILED)|NETWORK_CHANGED|EMPTY_RESPONSE|ADDRESS_UNREACHABLE|SOCKET_NOT_CONNECTED)/

/** The console-channel MIRROR of a transport failure. Chromium logs one of these
 *  for the same event it reports to `requestfailed` — this is the twin the
 *  classifier never muted, and therefore the one that gated. */
export const CONSOLE_TRANSPORT_MIRROR =
  /^Failed to load resource:\s*net::ERR_/i

/** A dynamic import that failed because its module never arrived. Reaching the
 *  ErrorBoundary makes it look like a product render crash; it is not. */
export const DYN_IMPORT_FAILURE = /Failed to fetch dynamically imported module/i

export const newRunId = () => crypto.randomUUID()

/**
 * Probe the gallery origin. Returns true iff it answers.
 * Deliberately an HTTP GET, not a TCP connect: a TCP probe passes against a
 * socket that is bound but whose server is wedged, and it misses a server bound
 * only to IPv6 localhost — both real cases here.
 */
export async function originAlive(url, timeoutMs = 10000) {
  try {
    const ac = new AbortController()
    const t = setTimeout(() => ac.abort(), timeoutMs)
    const r = await fetch(url, { signal: ac.signal })
    clearTimeout(t)
    // Drain the body. An unconsumed undici response keeps its connection and
    // buffer alive until GC, and this runs every few seconds for the length of a
    // crawl — ~144 undrained gallery-HTML responses per run otherwise.
    try {
      await r.arrayBuffer()
    } catch {
      /* body already gone — we only needed the status */
    }
    return r.ok
  } catch {
    return false
  }
}

/**
 * Watch the origin for the duration of a crawl.
 *
 * `stop()` returns `{ everDown, checks, downAt, consecutiveFailures }`.
 * Sampling (rather than inferring purely from findings) matters because it
 * distinguishes "the origin died" from "the product genuinely broke" — the two
 * produce similar finding volumes and previously could not be told apart.
 *
 * ## Why a single failed sample is NOT enough
 *
 * A first version latched `everDown` on one failed probe, and `assessRun` then
 * VOIDed the whole run. But the thing being probed is a Vite dev server
 * simultaneously serving six concurrent Chromium pages; a cold module transform
 * or a GC pause can easily exceed one probe's timeout. One slow sample out of
 * ~144 would then discard a healthy twelve-minute crawl and hard-fail the gate —
 * a FALSE FAIL, which for a gate is a worse outcome than the noise it was
 * introduced to suppress, and it is the exact failure mode this module claims to
 * eliminate.
 *
 * So a death must be CONFIRMED: `requiredFailures` consecutive failed probes
 * (default 3, ~15 s of unbroken unreachability). A genuinely dead server fails
 * every probe forever and trips this immediately; a transient stall does not.
 * The first sample fires straight away rather than after `intervalMs`, so a
 * short crawl cannot finish with `checks: 0` and be reported "alive" without any
 * evidence at all.
 */
export function watchOrigin(url, { intervalMs = 5000, requiredFailures = 3 } = {}) {
  const state = {
    everDown: false,
    checks: 0,
    downAt: null,
    consecutiveFailures: 0,
    maxConsecutiveFailures: 0,
  }
  let stopped = false
  let timer = null
  const tick = async () => {
    if (stopped) return
    const ok = await originAlive(url)
    if (stopped) return // stop() raced us; do not mutate a snapshot already taken
    state.checks++
    if (ok) {
      state.consecutiveFailures = 0
    } else {
      state.consecutiveFailures++
      state.maxConsecutiveFailures = Math.max(
        state.maxConsecutiveFailures,
        state.consecutiveFailures,
      )
      if (state.consecutiveFailures >= requiredFailures && !state.everDown) {
        state.everDown = true
        state.downAt = new Date().toISOString()
      }
    }
    if (!stopped) timer = setTimeout(tick, intervalMs)
  }
  // Sample IMMEDIATELY, so `checks` is never 0 for a short run.
  timer = setTimeout(tick, 0)
  return {
    stop() {
      stopped = true
      clearTimeout(timer)
      return { ...state }
    },
  }
}

/** Count the three faces of a transport failure across a finding set. */
export function contaminationOf(findings) {
  let requestFailed = 0
  let consoleMirror = 0
  let dynImportCrash = 0
  for (const f of findings) {
    const d = f.detail || ''
    if (f.category === 'request-failed' && TRANSPORT_DEAD.test(d)) requestFailed++
    else if (f.category === 'console-error' && CONSOLE_TRANSPORT_MIRROR.test(d))
      consoleMirror++
    else if (
      (f.category === 'crash' || f.category === 'page-error') &&
      DYN_IMPORT_FAILURE.test(d)
    )
      dynImportCrash++
  }
  const total = requestFailed + consoleMirror + dynImportCrash
  return {
    requestFailed,
    consoleMirror,
    dynImportCrash,
    total,
    pct: findings.length ? Math.round((total / findings.length) * 1000) / 10 : 0,
  }
}

/**
 * Decide whether a completed crawl may be believed.
 *
 * ## Calibrating the contamination bar — BOTH a floor and a ratio
 *
 * A ratio alone is wrong in both directions, and both were real:
 *
 * - **False FAIL.** A handful of `net::ERR_ABORTED` on dev assets is ROUTINE —
 *   `runtime-health`'s own rationale block documents a full page reload aborting
 *   in-flight ESM/font imports, and those are already muted as harness noise by
 *   the gating formula. `origin/main`'s baseline carries 36 such findings out of
 *   608 (5.9%). A 10%-of-all-findings bar therefore sits uncomfortably close to
 *   normal, and a SHORT crawl (few findings) trips it on two artifacts.
 * - **False PASS.** The denominator includes hundreds of LOW `spacing-grid`
 *   items, so a large crawl can dilute 900 genuine transport errors under the
 *   same bar.
 *
 * The two populations are not close, so the bar does not need to be delicate —
 * it needs to be robust at BOTH ends. A run is VOID only when the artifacts pass
 * an absolute FLOOR (`minAbsolute`, 50 — well above the ~36 routine baseline)
 * **and** a generous RATIO (`maxPct`, 25%). Measured against every run this
 * change has data for:
 *
 * | run | artifacts / findings | verdict |
 * |---|---|---|
 * | origin/main baseline | 36 / 608 (5.9%) | valid — below the floor |
 * | clean full crawl (this box) | 0 / 531 | valid |
 * | HMR-disturbed crawl | 538 / 1025 (52.5%) | VOID — floor and ratio |
 * | reported field case | 10430 / 10925 (95.5%) | VOID |
 *
 * The origin watcher is the primary, independent detector; this is the backstop
 * for an origin that flapped between its samples.
 */
export function assessRun({
  findings,
  origin,
  cellsPlanned,
  cellsCompleted,
  maxPct = 25,
  minAbsolute = 50,
}) {
  const contamination = contaminationOf(findings)
  const reasons = []
  if (origin?.everDown)
    reasons.push(
      `the gallery origin was UNREACHABLE during the crawl (${origin.maxConsecutiveFailures ?? '?'} ` +
        `consecutive failed probes, first confirmed down at ${origin.downAt}) — every request in ` +
        `flight failed, so these findings describe the harness, not the product`,
    )
  if (contamination.total >= minAbsolute && contamination.pct > maxPct)
    reasons.push(
      `${contamination.total} of ${findings.length} findings (${contamination.pct}%) are transport ` +
        `artifacts (${contamination.requestFailed} failed requests, ${contamination.consoleMirror} console ` +
        `mirrors, ${contamination.dynImportCrash} dynamic-import crashes) — past BOTH the ${minAbsolute}-artifact ` +
        `floor and the ${maxPct}% ratio. A few aborted dev assets are routine; this is not.`,
    )
  if (cellsCompleted < cellsPlanned)
    reasons.push(
      `only ${cellsCompleted} of ${cellsPlanned} cells completed — the crawl did not finish`,
    )
  return { void: reasons.length > 0, reasons, contamination }
}

/**
 * Write the run manifest ATOMICALLY (temp + rename), and only after the crawl
 * has drained. A killed run therefore leaves NO manifest, which is exactly what
 * lets a consumer tell "this run produced nothing" from "a previous run produced
 * this" — the distinction `gate-ui` could not previously make.
 */
export function writeRunManifest(dir, manifest) {
  fs.mkdirSync(dir, { recursive: true })
  const finalPath = path.join(dir, 'RUNTIME_RUN.json')
  const tmp = `${finalPath}.${process.pid}.tmp`
  fs.writeFileSync(tmp, `${JSON.stringify(manifest, null, 2)}\n`)
  fs.renameSync(tmp, finalPath)
  return finalPath
}

export function readRunManifest(dir) {
  try {
    return JSON.parse(fs.readFileSync(path.join(dir, 'RUNTIME_RUN.json'), 'utf-8'))
  } catch {
    return null
  }
}

/** Remove any prior run's artifacts so nothing can be inherited. */
export function clearRunArtifacts(dir) {
  // RUNTIME_FINDINGS.md is included deliberately: it is the artifact humans and
  // agents actually READ (CLAUDE.md points at it). Leaving it behind meant a
  // killed crawl left the PREVIOUS run's report in place — the exact
  // stale-inheritance defect this feature exists to close, left wide open on the
  // one file anybody looks at.
  for (const f of ['RUNTIME_RUN.json', 'RUNTIME_FINDINGS.jsonl', 'RUNTIME_FINDINGS.md']) {
    try {
      fs.unlinkSync(path.join(dir, f))
    } catch {
      /* absent is fine — that is the desired end state */
    }
  }
}

/**
 * The consumer-side check `gate-ui` runs before it is allowed to roll anything
 * up. Returns `{ ok, reason }`; each refusal names its OWN cause, because
 * "something went wrong" is what made the stale roll-up hard to spot.
 */
export function verifyRunManifest(manifest, expectedRunId) {
  if (!manifest)
    return {
      ok: false,
      reason:
        'no run manifest — the runtime-health crawl did not complete (killed, crashed, or refused to start). ' +
        'Refusing to roll up findings, which would be a PREVIOUS run\'s data.',
    }
  if (manifest.complete !== true)
    return { ok: false, reason: `the crawl reported complete=${manifest.complete}` }
  if (expectedRunId && manifest.runId !== expectedRunId)
    return {
      ok: false,
      reason:
        `run id mismatch — the manifest is from run ${manifest.runId}, this run is ${expectedRunId}. ` +
        'That means the findings on disk belong to a DIFFERENT run.',
    }
  if (manifest.cellsCompleted < manifest.cellsPlanned)
    return {
      ok: false,
      reason: `only ${manifest.cellsCompleted} of ${manifest.cellsPlanned} cells completed — the crawl did not finish`,
    }
  if (manifest.void)
    return {
      ok: false,
      reason: `the run declared itself VOID: ${(manifest.voidReasons || []).join('; ')}`,
    }
  return { ok: true, reason: '' }
}
