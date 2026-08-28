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
 * React developer warnings. The harness taxonomy rates these MEDIUM — they are
 * real quality debt but not a gating runtime failure.
 *
 * These live HERE, next to the classifier, rather than in each crawl copy,
 * because the channel a warning arrives on is a REACT-VERSION detail and the
 * classification must not depend on it: React 19 routes `console.error` for
 * developer warnings that React 18 sent to `console.warn`. When the list and the
 * channel test were duplicated in each `runtime-health.mjs`, the taxonomy said
 * MEDIUM while the code recorded HIGH — a misclassification that gated every
 * surface carrying a stray `key` warning.
 */
/**
 * The subset that is UNAMBIGUOUSLY a React developer warning by its text alone.
 * Only these may be downgraded on the ERROR channel.
 *
 * The distinction is load-bearing. On the warning channel a false match costs
 * nothing — the message was already non-gating. On the error channel every false
 * match is a GATE HOLE, and the looser patterns below are ordinary English:
 * `/is deprecated/i` matches `GET /api/models failed: 410 Gone — this endpoint
 * is deprecated`, and `/findDOMNode/i` matches React 19's `TypeError:
 * ReactDOM.findDOMNode is not a function`, which is a real crash, not a warning.
 * Each pattern here names a specific React warning string that no plausible
 * application error contains.
 */
export const REACT_WARNING_STRICT = [
  // ONE pattern per React message. Two patterns for the same warning
  // (`/unique "key" prop/i` AND `/Each child in a list/i`) mask each other:
  // deleting either is a silent no-op, so neither is pinned by any test.
  // Quote-independent, because the runtime string embeds real quotes.
  /Each child in a list should have a unique/i,
  /not wrapped in act\(/i,
  // DOM nesting. React 19 REWROTE this warning, so matching only the React 18
  // spelling left the whole family gating HIGH against the very version the fix
  // was written for. Verified against the installed react-dom 19 dev build:
  // `In HTML, %s cannot be a child of <%s>.` / `… cannot be a descendant of …`,
  // and `cannot appear as a descendant` / `validateDOMNesting` appear ZERO times.
  // Both spellings are kept — a repo can be on either major.
  /In HTML, .{0,80}cannot be a (child|descendant) of/i, // React 19
  /validateDOMNesting/i, //                                React 18
  /cannot appear as a descendant of/i, //                  React 18
]

/**
 * The warning-channel list: the historical loose patterns PLUS the strict ones.
 *
 * Not quite "the historical list" — the strict set adds the React-19 DOM-nesting
 * texts and `validateDOMNesting`, so a handful of messages that previously went
 * unrecorded on the warning channel are now recorded as MEDIUM. That is a
 * deliberate widening in the NON-gating direction (ledger only); the loose
 * patterns are still never consulted on the error channel, which is the half
 * that decides whether the build fails.
 */
export const REACT_WARNING = [
  ...REACT_WARNING_STRICT,
  /^Warning:/,
  /not wrapped in act/i,
  /is deprecated/i,
  /cannot appear as a descendant/i,
  /findDOMNode/i,
]

/** Channels the crawl records at all. `log`/`info`/`debug` are not diagnostics —
 *  consulting the warning list before the channel would newly record ordinary
 *  app logging as findings. */
const DIAGNOSTIC_CHANNELS = new Set(['error', 'warning', 'warn'])

/**
 * Severity of a raw console-error / uncaught exception, given the state it fired
 * in. The `error` state DELIBERATELY makes the mock API return 500s to exercise
 * each surface's failure handling, so a store logging / rethrowing that injected
 * failure is EXPECTED there — MEDIUM (the tracked "error-handling gap" backlog),
 * not a gating defect. An ErrorBoundary crash is HIGH regardless. In every other
 * state a console-error / pageerror is an unexpected runtime defect → HIGH.
 */
export function errSeverity(state, text) {
  if (ERRORBOUNDARY.test(text)) return 'HIGH'
  return state === 'error' ? 'MEDIUM' : 'HIGH'
}

/**
 * Classify a console message into `{category, severity}`, or `null` when it is
 * not recorded at all.
 *
 * **Channel-independent by design.** The message TYPE (`error` / `warning`) only
 * decides whether a NON-React message is recorded; it never decides a React
 * warning's severity. A blanket downgrade of the error channel would blind the
 * gate, so the arms are ordered: an ErrorBoundary crash outranks everything, a
 * known React warning is MEDIUM on any channel, and anything else on the error
 * channel keeps its state-aware HIGH.
 *
 * @param type   playwright's `msg.type()` — 'error' | 'warning' | 'warn' | …
 * @param state  the gallery cell state ('loaded' | 'empty' | 'error' | …)
 * @param text   the message text
 */
export function classifyConsoleMessage(type, state, text) {
  // Channel-independent BETWEEN the two diagnostic channels — NOT channel-blind.
  // `log`/`info`/`debug` stay unrecorded, as they always were.
  if (!DIAGNOSTIC_CHANNELS.has(type)) return null

  if (type === 'error') {
    // A render crash outranks everything, including a message that also carries
    // React-warning text (an ErrorBoundary report quotes the error it caught).
    if (ERRORBOUNDARY.test(text)) return { category: 'crash', severity: 'HIGH' }
    // THE FIX: a React developer warning is MEDIUM even though React 19 emits it
    // here. Narrow list only — on this channel a false match is a gate hole.
    if (matchesAny(text, REACT_WARNING_STRICT))
      return { category: 'react-warning', severity: 'MEDIUM' }
    return { category: 'console-error', severity: errSeverity(state, text) }
  }

  // Warning channel — the loose list; over-matching here is non-gating.
  if (matchesAny(text, REACT_WARNING))
    return { category: 'react-warning', severity: 'MEDIUM' }
  return null
}

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
