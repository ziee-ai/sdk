/**
 * FINDING CLASSIFICATION — which HIGH findings are gallery-harness artifacts
 * rather than product defects.
 *
 * Extracted into a shared module for two reasons:
 *
 * 1. **Testability.** It used to live inside `runtime-health.mjs`, which runs a
 *    crawl on import, so it could not be unit-tested at all. Its two newest arms
 *    are precisely the ones that can BLIND the gate — a blind reviewer confirmed
 *    that blanket-muting both left every test green — so they need direct,
 *    behavioural tests, both for what they mute and for what they must NOT.
 * 2. **Parity.** The harness exists in two live copies. Duplicating this logic is
 *    exactly the drift that let the defect class survive repeated fixing; both
 *    copies now import this one implementation.
 */
import {
  CONSOLE_TRANSPORT_MIRROR,
  DYN_IMPORT_FAILURE,
  TRANSPORT_DEAD,
} from './run-validity.mjs'

/** An ErrorBoundary catch — a genuine render CRASH in any state. */
export const ERRORBOUNDARY = /\[AppErrorBoundary/

/**
 * A failed request to a Vite-served dev asset (source module, `/@`-internal,
 * node_modules font/chunk) — dev transport, never a product fetch. Product data
 * goes to `/api`, which the mock cassette intercepts before the network.
 */
export const isViteDevAsset = url => {
  if (!url) return false
  // Match the PATHNAME only. Testing the whole URL let a PRODUCT endpoint be
  // classified as a dev asset because its QUERY contained a source-file-shaped
  // string — e.g. `/api/x?next=/src/foo.tsx`. That matters twice over: such a
  // finding would be muted (hiding a real product failure), and it would also
  // corroborate a crash-mute it is no evidence for.
  const p = pathnameOf(url)
  return (
    /\/@(fs|id|vite|react-refresh)\b/.test(p) ||
    /\/node_modules\//.test(p) ||
    /\/(src|modules|core|api-client|dev|components|hooks|widgets|stores|events|utils|assets)\/.*\.(tsx?|jsx?|mjs|css)$/.test(
      p,
    )
  )
}

const pathnameOf = u => {
  try {
    return new URL(u, 'http://localhost').pathname
  } catch {
    return String(u).split('?')[0]
  }
}

/** The URL inside a `request-failed` detail (`GET <url> — <err>`). */
export const requestUrlOf = detail =>
  /(?:GET|POST|PUT|DELETE|PATCH|HEAD)\s+(\S+)/.exec(detail || '')?.[1] ?? null

/** Compare module URLs by PATH only — the query (`?t=…`, `?import`) differs
 *  between a request record and the import-failure message for one module. */
export const normalizeAssetUrl = u => pathnameOf(u)

/**
 * The documented, NARROW console-text allowlist. Deliberately short: a real React
 * hydration/DOM-nesting error, a thrown app TypeError, or a genuine broken
 * PRODUCT fetch matches none of these and still gates.
 */
export const HARNESS_CONSOLE = [
  // The console MIRROR of an HTTP status. In the gallery every /api response is
  // the mock cassette's deliberate output (the `error` state injects 500/403 on
  // purpose) — the request/HTTP layer, not application logic.
  /^Failed to load resource: the server responded with a status of/i,
  // The cassette's own reject() sentinel, used by `*-err` surfaces by design.
  /Gallery forced error/i,
  // The gallery mock returns a plain object for a file body, so File.store's
  // `.text()` throws; production returns a real Blob. A mock-FIXTURE shape gap.
  /\[FileStore\] Failed to load file text content:.*\.text is not a function/i,
]

const matchesAny = (s, list) => list.some(re => re.test(s))

/**
 * Per-cell evidence gathered before classification. The crash arm needs to know
 * what ELSE the same cell saw, which is only knowable once the cell is finished —
 * which is why classification is a post-crawl pass rather than done at emit time.
 */
export function cellEvidence(findings) {
  const cells = new Map()
  const keyOf = f => `${f.surface}|${f.state}|${f.theme}`
  for (const f of findings) {
    const k = keyOf(f)
    const c = cells.get(k) || { failedModuleUrls: new Set() }
    if (f.category === 'request-failed' && TRANSPORT_DEAD.test(f.detail || '')) {
      // Extract the URL rather than testing the WHOLE detail string: a PRODUCT
      // url whose error text merely CONTAINS a dev-asset-shaped substring (a
      // `?next=/src/foo.tsx`, a chained url) would otherwise corroborate a
      // crash-mute it is not evidence for — making the corroboration signal
      // looser than the mute it authorises.
      const url = requestUrlOf(f.detail)
      if (url && isViteDevAsset(url)) c.failedModuleUrls.add(normalizeAssetUrl(url))
    }
    cells.set(k, c)
  }
  return { cells, keyOf }
}

/**
 * True when a HIGH finding is documented gallery-harness noise (subtracted from
 * the gating total, but still emitted and listed in its own rollup section).
 *
 * @param f     the finding
 * @param cell  that cell's evidence from `cellEvidence`
 */
export function isHarnessNoise(f, cell) {
  const d = f.detail || ''

  if (f.category === 'console-error') {
    if (matchesAny(d, HARNESS_CONSOLE)) return true
    // The PAIRED transport-mirror arm. Chromium reports ONE transport failure on
    // BOTH channels; the `request-failed` twin is muted below as a dev asset, so
    // muting only that one left the same single event half-gating — which is how
    // a disturbed run produced thousands of gating HIGHs.
    //
    // Keyed on the console message's OWN resource url (`msg.location().url`),
    // NOT on its text, which carries no url. That is what keeps it narrow: the
    // SAME error text against a PRODUCT url is not muted and still gates. Going
    // blind to genuine transport failure would be a worse defect than the one
    // being fixed.
    if (CONSOLE_TRANSPORT_MIRROR.test(d))
      return Boolean(f.resourceUrl) && isViteDevAsset(f.resourceUrl)
    return false
  }

  if (f.category === 'request-failed') {
    const url = requestUrlOf(d)
    return isViteDevAsset(url ?? d)
  }

  // A dynamic import that never arrived rejects and trips the app's
  // ErrorBoundary, so a harness disturbance FABRICATES render crashes. This is
  // the ONLY circumstance in which a crash is muted, and the corroboration is
  // deliberately strict: the SAME MODULE must have been observed failing at the
  // transport layer in this cell. A weaker rule ("this cell saw any dev-asset
  // abort") would mute a GENUINELY broken dynamic import — a bad lazy() path, a
  // chunk the product cannot fetch — because a routine reload-abort happened to
  // occur alongside it, and the surface would report PASS while rendering
  // nothing but an ErrorBoundary.
  if (f.category === 'crash' || f.category === 'page-error') {
    if (!DYN_IMPORT_FAILURE.test(d)) return false
    const url = /imported module:\s*(\S+)/.exec(d)?.[1]
    if (!url || !isViteDevAsset(url)) return false
    return Boolean(cell?.failedModuleUrls?.has(normalizeAssetUrl(url)))
  }

  return false
}

/**
 * Stamp `baselined` / `harness` on every HIGH finding, ONCE, after the crawl.
 * Mutates and returns `findings`.
 */
export function classifyAll(findings, isRuntimeBaselined = () => false) {
  const { cells, keyOf } = cellEvidence(findings)
  for (const f of findings) {
    if (f.severity !== 'HIGH') continue
    if (isRuntimeBaselined(f)) f.baselined = true
    else if (isHarnessNoise(f, cells.get(keyOf(f)))) f.harness = true
  }
  return findings
}
