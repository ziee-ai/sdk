/**
 * RUNTIME-HEALTH pass — the systematized version of "render every surface and
 * watch what the browser complains about".
 *
 * This is the pass that originally caught the /settings/user-groups crash by
 * hand; it now runs automatically across EVERY gallery surface × state × theme
 * and captures, per cell:
 *
 *   • console errors               (HIGH)   — real runtime errors
 *   • uncaught exceptions          (HIGH)   — pageerror / ErrorBoundary crash
 *   • failed network requests      (HIGH)   — a surface asking for a broken asset
 *   • React warnings               (MEDIUM) — key/act/deprecation console.warn
 *   • WCAG-AA contrast failures    (HIGH)   — getComputedStyle fg-vs-effective-bg
 *   • interactive missing a11y name(MEDIUM) — button/link/field with no name
 *   • off-4px-grid spacing         (LOW)    — computed padding/margin/gap
 *
 * Surfaces enumerated: every `gallery-page-<slug>` from the browse canvas (in
 * loaded/empty/error) PLUS every overlay open-state (from the runtime
 * `window.__GALLERY_OVERLAYS__` manifest). Each cell is a full-page reload of the
 * URL-isolation entry (`?surface=&state=&theme=`) so global singletons never
 * bleed across cells.
 *
 * Output: RUNTIME_FINDINGS.jsonl (one finding per line, machine-readable — feeds
 * the evaluator gate) + RUNTIME_FINDINGS.md (grouped human summary).
 *
 * Usage:
 *   node scripts/runtime-health.mjs [--url=BASE] [--out=DIR]
 *        [--states=loaded,empty,error] [--themes=light,dark] [--report-only]
 *
 * Exit code: non-zero if any HIGH finding exists (unless --report-only). The
 * evaluator gate (gate-ui.mjs) runs it --report-only and does its own per-surface
 * pass/fail accounting from the JSONL.
 */
import { chromium } from '@playwright/test'
import fs from 'node:fs'
import path from 'node:path'
import { pathToFileURL } from 'node:url'
import { enumerateSurfaces } from './lib/gallery-surfaces.mjs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

// Config-driven anchors (was `../src/dev/gallery` + `runtime-baseline.js` +
// port 1420 hardcodes). Resolved from `gallery.config.json` in the app's cwd.
const CFG = resolveGalleryConfig()
const GALLERY_DIR = path.resolve(CFG.__cwd, CFG.galleryDir)

// The app's baselined-findings module (content) is injected via config; default
// to a no-op so a minimal app (no baseline) simply gates on every HIGH.
let isRuntimeBaselined = () => false
if (CFG.runtimeBaselineModule) {
  const mod = await import(
    pathToFileURL(path.resolve(CFG.__cwd, CFG.runtimeBaselineModule)).href
  )
  isRuntimeBaselined = mod.isRuntimeBaselined ?? (() => false)
}

const arg = (n, d) =>
  (process.argv.find(a => a.startsWith(`--${n}=`)) || `--${n}=${d}`)
    .split('=')
    .slice(1)
    .join('=')
const flag = n => process.argv.includes(`--${n}`)

const PORT = process.env.GALLERY_PORT || String(CFG.port)
const BASE = arg('url', `http://localhost:${PORT}${CFG.galleryUrl}`)
const OUT = arg('out', GALLERY_DIR)
const STATES = arg('states', 'loaded,empty,error').split(',').filter(Boolean)
const THEMES = arg('themes', 'light,dark').split(',').filter(Boolean)
const REPORT_ONLY = flag('report-only')
const CONCURRENCY = Number(arg('concurrency', '6'))

const SEVERITY = { HIGH: 3, MEDIUM: 2, LOW: 1 }

/** A benign console/request noise filter — these are NOT surface defects. */
const IGNORE_CONSOLE = [
  /Download the React DevTools/i,
  /\[vite\]/i,
  /React Router Future Flag/i,
  /Content-Security-Policy/i,
]
const IGNORE_REQUEST = [
  /favicon\.ico$/i,
  /\.map$/i,
  /^data:/i,
  /@vite\/client/i,
  /@react-refresh/i,
]
const REACT_WARNING = [
  /^Warning:/,
  /unique "key" prop/i,
  /not wrapped in act/i,
  /is deprecated/i,
  /Each child in a list/i,
  /cannot appear as a descendant/i,
  /findDOMNode/i,
]

/**
 * HARNESS-NOISE classifier — documented, NARROW patterns for console/request
 * findings that are gallery-harness / dev-server / mock-cassette artifacts, NOT
 * product defects. These are subtracted from the gating HIGH total (exactly like
 * the runtime-baseline) but STILL EMITTED to the JSONL and listed in a dedicated
 * rollup section — so the filter is fully auditable, never a silent mute.
 *
 * This is deliberately a SHORT allowlist, not a blanket. Anything that does NOT
 * match one of these — a real React hydration/DOM-nesting error, a thrown app
 * TypeError, an ErrorBoundary `crash` (which is NEVER muted here, regardless of
 * trigger), a genuine broken PRODUCT fetch — still gates.
 *
 * Rationale per class (from the R3 runtime triage — see fix/runtime-triage):
 *  • Browser "Failed to load resource: the server responded with a status of N"
 *    — the console-channel MIRROR of an HTTP status, carrying no app stack. In
 *    the gallery every /api response is the mock cassette's deliberate output
 *    (the `error` state injects 500/403 on purpose). This is the request/HTTP
 *    layer, not application logic — the console twin of `request-failed`.
 *  • "Gallery forced error" — the cassette's own reject() sentinel that
 *    `*-err` / error-state surfaces use to exercise error UI, by design.
 *  • FileStore `…text is not a function` — the gallery mock returns a plain
 *    object for a file body, so File.store's `.text()` throws; production
 *    returns a real Blob. A mock-FIXTURE shape gap, not a product bug.
 *  • Vite dev-asset request failures — a full reload aborts in-flight ESM /
 *    font imports (`/@fs/…/node_modules/katex/…woff2 — net::ERR_ABORTED`).
 *    Dev-server transport, never a product fetch (product data goes to /api,
 *    which the mock intercepts before the network).
 */
const HARNESS_CONSOLE = [
  /^Failed to load resource: the server responded with a status of/i,
  /Gallery forced error/i,
  /\[FileStore\] Failed to load file text content:.*\.text is not a function/i,
]
/** A failed request to a Vite-served dev asset (source module, /@-internal,
 *  node_modules font/chunk) — dev-transport noise, never a product fetch. */
const isViteDevAsset = url =>
  /\/@(fs|id|vite|react-refresh)\b/.test(url) ||
  /\/node_modules\//.test(url) ||
  /\/(src|modules|core|api-client|dev|components|hooks|widgets|stores|events|utils|assets)\/[^?]*\.(tsx?|jsx?|mjs|css)(\?|$)/.test(
    url,
  )
/**
 * True when a HIGH finding is documented gallery-harness noise (subtracted from
 * gating, but kept visible). A `crash` (ErrorBoundary) is intentionally excluded
 * — a render crash gates no matter what triggered it.
 */
function isHarnessNoise(f) {
  const d = f.detail || ''
  if (f.category === 'console-error' && matchesAny(d, HARNESS_CONSOLE)) return true
  if (f.category === 'request-failed') {
    const m = /(?:GET|POST|PUT|DELETE|PATCH|HEAD)\s+(\S+)/.exec(d)
    return isViteDevAsset(m ? m[1] : d)
  }
  return false
}

// An ErrorBoundary catch is a genuine render CRASH in ANY state (the surface
// failed to render, incl. its error UI) — always HIGH.
const ERRORBOUNDARY = /\[AppErrorBoundary/

/**
 * Severity of a raw console-error / uncaught exception, given the state it fired
 * in. The `error` state DELIBERATELY makes the mock API return 500s to exercise
 * each surface's failure handling, so a store logging / rethrowing that injected
 * failure is EXPECTED there — MEDIUM (the tracked "error-handling gap" backlog),
 * not a gating defect. An ErrorBoundary crash is HIGH regardless. In every other
 * state a console-error / pageerror is an unexpected runtime defect → HIGH.
 */
function errSeverity(state, text) {
  if (ERRORBOUNDARY.test(text)) return 'HIGH'
  return state === 'error' ? 'MEDIUM' : 'HIGH'
}

const matchesAny = (s, list) => list.some(re => re.test(s))

/**
 * In-page audit — contrast (WCAG AA), interactive accessible-name, off-4px-grid
 * spacing. Runs entirely in the browser against the rendered surface; returns a
 * flat list of {category, severity, detail, selector}. Gallery chrome
 * (`[data-gallery-chrome]`, the control bar, the per-surface frame header) is
 * excluded so only the SURFACE under test is judged.
 */
function inPageAudit() {
  const findings = []
  const GRID = 4

  const isChrome = el =>
    !!el.closest('[data-gallery-chrome]') ||
    !!el.closest('[data-testid="gallery-controls"]')

  const visible = el => {
    const cs = getComputedStyle(el)
    if (cs.visibility === 'hidden' || cs.display === 'none' || cs.opacity === '0')
      return false
    const r = el.getBoundingClientRect()
    return r.width >= 1 && r.height >= 1
  }

  const selectorFor = el => {
    const tid = el.getAttribute?.('data-testid')
    if (tid) return `[data-testid="${tid}"]`
    const id = el.id ? `#${el.id}` : ''
    const cls =
      typeof el.className === 'string' && el.className
        ? '.' + el.className.trim().split(/\s+/).slice(0, 2).join('.')
        : ''
    return `${el.tagName.toLowerCase()}${id}${cls}`.slice(0, 80)
  }

  // ---- color helpers ------------------------------------------------------
  // Parse ANY CSS color (rgb/hex/named AND modern oklch()/oklab()/color()) to
  // RGBA via a 1×1 canvas — the kit's theme tokens compute to oklch(), which a
  // naive rgb() regex misses (that made every dark-theme background resolve to
  // the white fallback → false-positive contrast failures).
  const cvs = document.createElement('canvas')
  cvs.width = cvs.height = 1
  const cctx = cvs.getContext('2d', { willReadFrequently: true })
  const colorCache = new Map()
  const parseColor = c => {
    if (!c || c === 'transparent') return { r: 0, g: 0, b: 0, a: 0 }
    if (colorCache.has(c)) return colorCache.get(c)
    let out = null
    try {
      cctx.clearRect(0, 0, 1, 1)
      cctx.fillStyle = '#000'
      cctx.fillStyle = c // invalid strings leave it '#000' → detectable below
      cctx.fillRect(0, 0, 1, 1)
      const [r, g, b, aByte] = cctx.getImageData(0, 0, 1, 1).data
      out = { r, g, b, a: aByte / 255 }
    } catch {
      out = null
    }
    colorCache.set(c, out)
    return out
  }
  const over = (fg, bg) => {
    // composite fg (with alpha) over an opaque bg
    const a = fg.a
    return {
      r: fg.r * a + bg.r * (1 - a),
      g: fg.g * a + bg.g * (1 - a),
      b: fg.b * a + bg.b * (1 - a),
      a: 1,
    }
  }
  const lum = ({ r, g, b }) => {
    const f = v => {
      const s = v / 255
      return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
    }
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
  }
  const ratio = (a, b) => {
    const l1 = lum(a)
    const l2 = lum(b)
    return (Math.max(l1, l2) + 0.05) / (Math.min(l1, l2) + 0.05)
  }
  // Page base = the actual computed background of <html>/<body> (theme-aware),
  // falling back to white/near-black by theme only if both are transparent.
  const rootBgColor = () => {
    for (const el of [document.documentElement, document.body]) {
      const c = parseColor(getComputedStyle(el).backgroundColor)
      if (c && c.a > 0) return { r: c.r, g: c.g, b: c.b, a: 1 }
    }
    return document.documentElement.classList.contains('dark')
      ? { r: 10, g: 10, b: 10, a: 1 }
      : { r: 255, g: 255, b: 255, a: 1 }
  }
  const PAGE_BASE = rootBgColor()

  /** Resolve the effective (composited, opaque) background behind an element. */
  const effectiveBg = el => {
    let base = { ...PAGE_BASE } // theme-aware page default
    const stack = []
    let node = el
    while (node && node.nodeType === 1) {
      const bg = parseColor(getComputedStyle(node).backgroundColor)
      if (bg && bg.a > 0) stack.push(bg)
      node = node.parentElement
    }
    // composite from the furthest ancestor down to the nearest
    for (let i = stack.length - 1; i >= 0; i--) base = over(stack[i], base)
    return base
  }

  // ---- 1. contrast (visible text-bearing leaves) --------------------------
  const seenContrast = new Set()
  const textEls = Array.from(document.querySelectorAll('body *')).filter(el => {
    if (isChrome(el) || !visible(el)) return false
    // element must hold a direct, non-empty text node
    return Array.from(el.childNodes).some(
      n => n.nodeType === 3 && n.textContent.trim().length > 1,
    )
  })
  for (const el of textEls) {
    const cs = getComputedStyle(el)
    const fgRaw = parseColor(cs.color)
    if (!fgRaw) continue
    const bg = effectiveBg(el)
    const fg = fgRaw.a < 1 ? over(fgRaw, bg) : fgRaw
    const cr = ratio(fg, bg)
    const size = parseFloat(cs.fontSize)
    const bold = parseInt(cs.fontWeight, 10) >= 700
    const large = size >= 24 || (size >= 18.66 && bold)
    const threshold = large ? 3.0 : 4.5
    if (cr + 0.05 < threshold) {
      const key = `${cs.color}|${JSON.stringify(bg)}|${threshold}`
      if (seenContrast.has(key)) continue
      seenContrast.add(key)
      findings.push({
        category: 'contrast',
        severity: 'HIGH',
        selector: selectorFor(el),
        detail: `contrast ${cr.toFixed(2)}:1 < WCAG AA ${threshold}:1 (${large ? 'large' : 'normal'} text, ${size}px) — fg ${cs.color} on bg rgb(${Math.round(bg.r)},${Math.round(bg.g)},${Math.round(bg.b)})`,
      })
    }
  }

  // ---- 2. interactive elements missing an accessible name -----------------
  const accName = el => {
    const aria = el.getAttribute('aria-label')
    if (aria && aria.trim()) return aria.trim()
    const labelledby = el.getAttribute('aria-labelledby')
    if (labelledby) {
      const t = labelledby
        .split(/\s+/)
        .map(id => document.getElementById(id)?.textContent?.trim() || '')
        .join(' ')
        .trim()
      if (t) return t
    }
    const title = el.getAttribute('title')
    if (title && title.trim()) return title.trim()
    if (el.tagName === 'INPUT') {
      const ph = el.getAttribute('placeholder')
      if (ph && ph.trim()) return ph.trim()
      const val = el.getAttribute('value')
      if (
        val &&
        val.trim() &&
        ['submit', 'button', 'reset'].includes(el.getAttribute('type') || '')
      )
        return val.trim()
      if (el.id) {
        const lbl = document.querySelector(`label[for="${CSS.escape(el.id)}"]`)
        if (lbl?.textContent?.trim()) return lbl.textContent.trim()
      }
    }
    const wrapLabel = el.closest('label')
    if (wrapLabel?.textContent?.trim()) return wrapLabel.textContent.trim()
    const text = el.textContent?.trim()
    if (text) return text
    const img = el.querySelector('img[alt]')
    if (img?.getAttribute('alt')?.trim()) return img.getAttribute('alt').trim()
    return ''
  }
  const INTERACTIVE =
    'button, a[href], input:not([type="hidden"]), select, textarea, [role="button"], [role="link"], [role="checkbox"], [role="switch"], [role="tab"], [role="menuitem"], [role="radio"]'
  const seenName = new Set()
  for (const el of document.querySelectorAll(INTERACTIVE)) {
    if (isChrome(el) || !visible(el)) continue
    if (el.getAttribute('aria-hidden') === 'true') continue
    if (el.tagName === 'INPUT' && el.getAttribute('type') === 'radio') continue // grouped name handled elsewhere
    if (accName(el)) continue
    const sel = selectorFor(el)
    if (seenName.has(sel)) continue
    seenName.add(sel)
    findings.push({
      category: 'a11y-name',
      severity: 'MEDIUM',
      selector: sel,
      detail: `interactive <${el.tagName.toLowerCase()}${el.getAttribute('role') ? ` role=${el.getAttribute('role')}` : ''}> has no accessible name (no text/aria-label/title/label)`,
    })
  }

  // ---- 3. off-4px-grid spacing (informational) ----------------------------
  // The kit legitimately uses Tailwind 2px half-steps, so this is LOW: reported
  // as an aggregate count per surface, never gating. It surfaces DRIFT, not bugs.
  const offGrid = new Set()
  for (const el of document.querySelectorAll('body *')) {
    if (isChrome(el) || !visible(el)) continue
    const cs = getComputedStyle(el)
    const vals = [
      cs.paddingTop,
      cs.paddingRight,
      cs.paddingBottom,
      cs.paddingLeft,
      cs.marginTop,
      cs.marginRight,
      cs.marginBottom,
      cs.marginLeft,
      cs.rowGap,
      cs.columnGap,
    ]
    for (const v of vals) {
      const px = parseFloat(v)
      if (!px || Number.isNaN(px)) continue
      const abs = Math.abs(px)
      if (abs % GRID > 0.5 && GRID - (abs % GRID) > 0.5) offGrid.add(abs)
    }
  }
  if (offGrid.size) {
    const list = [...offGrid].sort((a, b) => a - b).slice(0, 12)
    findings.push({
      category: 'spacing-grid',
      severity: 'LOW',
      selector: 'body',
      detail: `${offGrid.size} distinct off-${GRID}px-grid spacing value(s): ${list.map(v => v + 'px').join(', ')}${offGrid.size > 12 ? ' …' : ''}`,
    })
  }

  return findings
}

// ---------------------------------------------------------------------------
async function main() {
  const browser = await chromium.launch()

  // 1. Enumerate EVERY surface class from the single source (browse render).
  const enumPage = await browser.newPage({
    viewport: { width: 1280, height: 900 },
  })
  const classes = await enumerateSurfaces(enumPage, BASE)
  await enumPage.close()

  // 2. Build the surface × state matrix. Pages get the data-state set; the
  //    interaction-only classes (overlay/deep/seeded) render once via
  //    `?surface=<slug>` (state is ignored by the mock for those).
  const cells = []
  for (const slug of classes.pages)
    for (const state of STATES) cells.push({ surface: slug, state, kind: 'page' })
  for (const slug of classes.overlays)
    cells.push({ surface: slug, state: 'open', kind: 'overlay' })
  for (const slug of classes.deep)
    cells.push({ surface: slug, state: 'deep', kind: 'deep' })
  for (const slug of classes.seeded)
    cells.push({ surface: slug, state: 'seeded', kind: 'seeded' })

  // Optional scoping: `--only-kinds=overlay` (comma-list) restricts the run to
  // one or more surface classes; `--only-match=<substr>` restricts to slugs
  // containing the substring. Lets an overlay-focused pass skip the ~300 pages.
  const onlyKinds = arg('only-kinds', '').split(',').filter(Boolean)
  const onlyMatch = arg('only-match', '')
  // COPY, do not alias: `scoped = cells` would share the array reference, so the
  // `cells.length = 0` below empties BOTH and the re-push restores nothing — the
  // default (no --only-*) run then renders ZERO cells and truthfully reports
  // "0 findings", a silent false-clean. `[...cells]` gives scoped its own array.
  let scoped = [...cells]
  if (onlyKinds.length) scoped = scoped.filter(c => onlyKinds.includes(c.kind))
  if (onlyMatch) scoped = scoped.filter(c => c.surface.includes(onlyMatch))
  cells.length = 0
  cells.push(...scoped)

  console.log(
    `runtime-health: ${classes.pages.length} pages × ${STATES.length} states + ${classes.overlays.length} overlays + ${classes.deep.length} deep + ${classes.seeded.length} seeded = ${cells.length} surface/state cells × ${THEMES.length} themes\n`,
  )

  // Fail-loud regression guard: enumerating surfaces but building ZERO cells is
  // the cell-aliasing false-clean class — it would report "0 findings" while
  // rendering nothing. If any surface class was enumerated and no scoping arg
  // was supplied, an empty cell list is a bug, not a clean run. Refuse to lie.
  const enumeratedSurfaces =
    classes.pages.length + classes.overlays.length + classes.deep.length + classes.seeded.length
  const scopingApplied = onlyKinds.length > 0 || onlyMatch.length > 0
  if (enumeratedSurfaces > 0 && cells.length === 0 && !scopingApplied) {
    console.error(
      `runtime-health: enumerated ${enumeratedSurfaces} surfaces but built 0 cells with no scoping — ` +
        `refusing to report a false "0 findings" (cell-aliasing regression). See the COPY comment above.`,
    )
    process.exit(2)
  }

  const findings = []
  // Normalize volatile substrings so the committed report is stable across runs:
  // the vite dev port (localhost:1477 vs :1420) and HMR cache-bust timestamps
  // (`?t=1783…`) otherwise churn every detail string.
  const normalizeDetail = d =>
    typeof d === 'string'
      ? d.replace(/localhost:\d+/g, 'localhost').replace(/\?t=\d+/g, '')
      : d
  const record = (cell, theme, f) => {
    if (f.detail) f.detail = normalizeDetail(f.detail)
    const finding = { ...cell, theme, ...f }
    // A documented pre-existing item (runtime-baseline.js) is still emitted, but
    // flagged so it does NOT count toward the gating HIGH total.
    if (finding.severity === 'HIGH' && isRuntimeBaselined(finding))
      finding.baselined = true
    // Documented gallery-harness noise (mock-cassette / dev-server artifacts) —
    // also emitted + visible, also subtracted from gating. Never masks a crash.
    else if (finding.severity === 'HIGH' && isHarnessNoise(finding))
      finding.harness = true
    findings.push(finding)
  }

  // Flatten to (cell, theme) jobs, then drain with a fixed-size worker pool —
  // each job is a fresh isolated page (own listeners, own singleton state), so
  // they parallelize cleanly against the one dev server.
  const jobs = []
  for (const cell of cells) for (const theme of THEMES) jobs.push({ cell, theme })
  const total = jobs.length
  let done = 0

  async function runJob({ cell, theme }) {
    const p = await browser.newPage({ viewport: { width: 1280, height: 900 } })
    p.on('console', m => {
      const t = m.text()
      if (matchesAny(t, IGNORE_CONSOLE)) return
      if (m.type() === 'error')
        record(cell, theme, {
          category: ERRORBOUNDARY.test(t) ? 'crash' : 'console-error',
          severity: errSeverity(cell.state, t),
          selector: null,
          detail: t.replace(/\s+/g, ' ').slice(0, 300),
        })
      else if (
        (m.type() === 'warning' || m.type() === 'warn') &&
        matchesAny(t, REACT_WARNING)
      )
        record(cell, theme, {
          category: 'react-warning',
          severity: 'MEDIUM',
          selector: null,
          detail: t.replace(/\s+/g, ' ').slice(0, 300),
        })
    })
    p.on('pageerror', e => {
      const msg = (e.message || String(e)).replace(/\s+/g, ' ').slice(0, 300)
      record(cell, theme, {
        category: 'page-error',
        severity: errSeverity(cell.state, msg),
        selector: null,
        detail: msg,
      })
    })
    p.on('requestfailed', req => {
      const url = req.url()
      if (matchesAny(url, IGNORE_REQUEST)) return
      record(cell, theme, {
        category: 'request-failed',
        severity: 'HIGH',
        selector: null,
        detail: `${req.method()} ${url} — ${req.failure()?.errorText ?? 'failed'}`,
      })
    })

    const url = `${BASE}?surface=${cell.surface}&state=${cell.state}&theme=${theme}`
    try {
      await p.goto(url, { waitUntil: 'domcontentloaded', timeout: 20_000 })
      // deep/seeded surfaces run a mount-time store seed (a few seconds) before
      // their reviewable state settles; pages/overlays settle fast.
      const settle =
        cell.kind === 'deep' || cell.kind === 'seeded'
          ? 2600
          : cell.state === 'error'
            ? 1100
            : 800
      await p.waitForTimeout(settle)
      const audit = await p.evaluate(inPageAudit)
      for (const f of audit) record(cell, theme, f)
    } catch (e) {
      record(cell, theme, {
        category: 'nav-error',
        severity: 'HIGH',
        selector: null,
        detail: (e.message || String(e)).slice(0, 200),
      })
    }
    await p.close()
    done++
    if (done % 25 === 0 || done === total)
      console.log(`  … ${done}/${total} cells`)
  }

  // Fixed-size pool: N long-lived workers each pull the next job off the queue.
  let next = 0
  const worker = async () => {
    while (next < jobs.length) {
      const job = jobs[next++]
      await runJob(job)
    }
  }
  await Promise.all(
    Array.from({ length: Math.min(CONCURRENCY, jobs.length) }, worker),
  )
  await browser.close()

  // 3. Write the outputs.
  fs.mkdirSync(OUT, { recursive: true })
  const jsonlPath = path.join(OUT, 'RUNTIME_FINDINGS.jsonl')
  fs.writeFileSync(
    jsonlPath,
    findings.map(f => JSON.stringify(f)).join('\n') + (findings.length ? '\n' : ''),
  )

  const byCat = {}
  for (const f of findings) byCat[f.category] = (byCat[f.category] || 0) + 1
  const bySev = { HIGH: 0, MEDIUM: 0, LOW: 0 }
  for (const f of findings) bySev[f.severity]++
  // Gating HIGH = HIGH minus documented-baselined AND documented-harness items.
  const baselinedCount = findings.filter(f => f.baselined).length
  const harnessCount = findings.filter(f => f.harness).length
  const gatingHigh = bySev.HIGH - baselinedCount - harnessCount
  // Harness-noise breakdown by category, for the transparency rollup.
  const harnessByCat = {}
  for (const f of findings)
    if (f.harness) harnessByCat[f.category] = (harnessByCat[f.category] || 0) + 1

  // Per-surface roll-up (worst severity per surface, across states/themes). A
  // baselined HIGH goes into `baselined`, NOT `high`, so it doesn't fail a surface.
  const surfaces = {}
  const blank = () => ({ high: 0, medium: 0, low: 0, baselined: 0, harness: 0 })
  for (const c of cells) surfaces[c.surface] ??= blank()
  for (const f of findings) {
    const s = (surfaces[f.surface] ??= blank())
    if (f.baselined) s.baselined++
    else if (f.harness) s.harness++
    else s[f.severity.toLowerCase()]++
  }
  const failingSurfaces = Object.entries(surfaces).filter(([, s]) => s.high > 0)

  const md = []
  md.push('# Runtime-health findings\n')
  md.push(
    `Generated by \`npm run gallery:runtime\` over ${cells.length} surface/state cells × ${THEMES.length} themes (${cells.length * THEMES.length} page loads). Each cell is a full reload of \`?surface=&state=&theme=\`; the browser's own diagnostics (console/pageerror/requestfailed) plus getComputedStyle contrast + a11y-name + 4px-grid checks are captured per cell.\n`,
  )
  md.push('## Totals\n')
  md.push('| Severity | Count |')
  md.push('|---|---|')
  md.push(`| 🔴 HIGH (gating) | ${gatingHigh} |`)
  md.push(`| 🟣 HIGH (harness-noise, non-gating) | ${harnessCount} |`)
  md.push(`| 🔵 HIGH (baselined, non-gating) | ${baselinedCount} |`)
  md.push(`| 🟡 MEDIUM | ${bySev.MEDIUM} |`)
  md.push(`| ⚪ LOW (informational) | ${bySev.LOW} |`)
  md.push(`| **Total** | **${findings.length}** |\n`)
  md.push('## By category\n')
  md.push('| Category | Count | Severity |')
  md.push('|---|---|---|')
  const catSev = {
    crash: 'HIGH',
    'console-error': 'HIGH*',
    'page-error': 'HIGH*',
    'request-failed': 'HIGH',
    'nav-error': 'HIGH',
    contrast: 'HIGH',
    'react-warning': 'MEDIUM',
    'a11y-name': 'MEDIUM',
    'spacing-grid': 'LOW',
  }
  for (const [cat, n] of Object.entries(byCat).sort((a, b) => b[1] - a[1]))
    md.push(`| \`${cat}\` | ${n} | ${catSev[cat] ?? '?'} |`)
  md.push('')
  md.push(
    '_\\* `console-error` / `page-error` are HIGH in loaded/empty/open states but MEDIUM in the `error` state (the deliberate 500-injection state — a store logging the injected failure is expected there; a render CRASH still upgrades to HIGH via the `crash` category)._\n',
  )
  md.push('## Surfaces with HIGH findings (gate-failing)\n')
  if (!failingSurfaces.length) {
    md.push(
      '_None — every surface is runtime-clean of gating HIGH findings._\n',
    )
  } else {
    md.push('| Surface | HIGH | MEDIUM | LOW |')
    md.push('|---|---|---|---|')
    for (const [s, c] of failingSurfaces.sort((a, b) => b[1].high - a[1].high))
      md.push(`| \`${s}\` | ${c.high} | ${c.medium} | ${c.low} |`)
    md.push('')
  }

  // Baselined (documented pre-existing) — visible but non-gating.
  const baselined = findings.filter(f => f.baselined)
  if (baselined.length) {
    md.push('## Baselined (documented pre-existing — non-gating)\n')
    md.push(
      'These are real findings held in `src/dev/gallery/runtime-baseline.js` with a triage note; the gate subtracts them so it fails only on NEW regressions. Re-triage before removing a baseline entry.\n',
    )
    const seen = new Set()
    for (const f of baselined) {
      const k = `${f.surface}|${f.detail}`
      if (seen.has(k)) continue
      seen.add(k)
      md.push(`- 🔵 **${f.category}** \`${f.surface}\` — ${f.detail}`)
    }
    md.push('')
  }

  // Harness-noise (documented mock-cassette / dev-server artifacts) — visible
  // but non-gating, subtracted from the HIGH total. This is the transparency
  // ledger for the filter: every muted finding is accounted for here by class.
  if (harnessCount) {
    md.push('## Harness-noise (documented, non-gating)\n')
    md.push(
      `${harnessCount} HIGH finding(s) classified as gallery-harness / dev-server / mock-cassette artifacts by \`isHarnessNoise()\` in \`scripts/runtime-health.mjs\` (see the rationale block there). Subtracted from the gating HIGH total but still emitted to the JSONL. A real console error / DOM-nesting error / ErrorBoundary crash does NOT match these patterns and still gates.\n`,
    )
    md.push('| Category | Muted count |')
    md.push('|---|---|')
    for (const [cat, n] of Object.entries(harnessByCat).sort((a, b) => b[1] - a[1]))
      md.push(`| \`${cat}\` | ${n} |`)
    md.push('')
  }

  // Detail — HIGH + MEDIUM, grouped by surface (LOW + baselined + harness omitted
  // from this section; LOW is in the JSONL, baselined + harness have rollups above).
  md.push('## Detail (HIGH + MEDIUM)\n')
  const gating = findings.filter(
    f => f.severity !== 'LOW' && !f.baselined && !f.harness,
  )
  if (!gating.length) {
    md.push('_No HIGH or MEDIUM findings._\n')
  } else {
    const grouped = {}
    for (const f of gating) (grouped[f.surface] ??= []).push(f)
    for (const [surface, list] of Object.entries(grouped).sort()) {
      md.push(`### \`${surface}\`\n`)
      // dedupe identical (category, detail, state) across themes
      const seen = new Set()
      for (const f of list) {
        const k = `${f.category}|${f.detail}|${f.state}`
        if (seen.has(k)) continue
        seen.add(k)
        const icon = f.severity === 'HIGH' ? '🔴' : '🟡'
        md.push(
          `- ${icon} **${f.category}** [${f.state}] ${f.selector ? `\`${f.selector}\` — ` : ''}${f.detail}`,
        )
      }
      md.push('')
    }
  }
  const mdPath = path.join(OUT, 'RUNTIME_FINDINGS.md')
  fs.writeFileSync(mdPath, md.join('\n'))

  console.log(
    `\n=== runtime-health: ${findings.length} findings (HIGH ${gatingHigh} gating${harnessCount ? ` + ${harnessCount} harness-noise` : ''}${baselinedCount ? ` + ${baselinedCount} baselined` : ''} / MEDIUM ${bySev.MEDIUM} / LOW ${bySev.LOW}) ===`,
  )
  if (harnessCount)
    console.log(
      `  (harness-noise muted: ${Object.entries(harnessByCat).map(([c, n]) => `${c} ${n}`).join(', ')})`,
    )
  console.log(`  ${failingSurfaces.length} surface(s) with gating HIGH findings`)
  console.log(`  → ${path.relative(process.cwd(), mdPath)}`)
  console.log(`  → ${path.relative(process.cwd(), jsonlPath)}`)

  if (!REPORT_ONLY && gatingHigh > 0) process.exitCode = 1
}

main().catch(e => {
  console.error(e)
  process.exit(2)
})
