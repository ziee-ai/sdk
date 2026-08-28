/**
 * TEST — the harness-noise classifier (TEST-9 / TEST-10 / TEST-11).
 *
 * These arms decide what the UI gate is allowed to IGNORE, so every one is
 * asserted in BOTH directions. A blind reviewer demonstrated that blanket-muting
 * the two new arms left the whole suite green; each `must still gate` case below
 * exists to make that mutation impossible.
 *
 * Run: node --test scripts/lib/finding-classify.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  cellEvidence,
  classifyAll,
  isHarnessNoise,
  isViteDevAsset,
  normalizeAssetUrl,
  requestUrlOf,
  classifyConsoleMessage,
  REACT_WARNING_STRICT,
} from './finding-classify.mjs'

const F = (over = {}) => ({
  surface: 's',
  state: 'loaded',
  theme: 'light',
  category: 'console-error',
  severity: 'HIGH',
  detail: 'x',
  ...over,
})
/** Classify one finding in the context of a whole cell's findings. */
const classifyIn = (target, all = [target]) => {
  const { cells, keyOf } = cellEvidence(all)
  return isHarnessNoise(target, cells.get(keyOf(target)))
}

const DEV = 'http://localhost/@fs/repo/node_modules/katex/dist/katex.woff2'
const DEV_SRC = 'http://localhost/src/modules/chat/ChatPage.tsx?t=123'
const PRODUCT = 'http://localhost/api/conversations'

// ---------------------------------------------------------------------------
// TEST-9 — the console transport-mirror arm, BOTH directions.
// ---------------------------------------------------------------------------
test('TEST-9 a transport mirror for a DEV ASSET is muted', () => {
  const f = F({
    detail: 'Failed to load resource: net::ERR_NETWORK_CHANGED',
    resourceUrl: DEV,
  })
  assert.equal(classifyIn(f), true)
})

test('TEST-9b the SAME error text for a PRODUCT url still GATES', () => {
  // This is the INV-1 "do not go blind" control. Blanket-muting the arm — the
  // mutation a reviewer used — turns this red.
  const f = F({
    detail: 'Failed to load resource: net::ERR_NETWORK_CHANGED',
    resourceUrl: PRODUCT,
  })
  assert.equal(
    classifyIn(f),
    false,
    'a transport failure against a PRODUCT url is a real finding and must gate',
  )
})

test('TEST-9c a transport mirror with NO resource url still GATES', () => {
  // Fail closed: if we cannot tell what failed, we do not mute it.
  const f = F({ detail: 'Failed to load resource: net::ERR_ABORTED', resourceUrl: null })
  assert.equal(classifyIn(f), false)
})

test('TEST-9d the HTTP-status mirror keeps its own (pre-existing) mute', () => {
  const f = F({
    detail: 'Failed to load resource: the server responded with a status of 500',
    resourceUrl: PRODUCT,
  })
  assert.equal(classifyIn(f), true, 'the cassette injects 500s on purpose')
})

test('TEST-9e an ordinary product console error is NEVER muted', () => {
  for (const detail of [
    'TypeError: cannot read properties of undefined',
    'Warning: validateDOMNesting(...): <div> cannot appear as a descendant of <p>',
    'Internal React error: Expected static flag was missing.',
  ])
    assert.equal(classifyIn(F({ detail, resourceUrl: DEV })), false, detail)
})

// ---------------------------------------------------------------------------
// TEST-10 — the crash arm. A crash is muted ONLY with same-module corroboration.
// ---------------------------------------------------------------------------
const crash = url =>
  F({
    category: 'crash',
    detail: `[AppErrorBoundary] Failed to fetch dynamically imported module: ${url}`,
  })
const abortedRequest = url =>
  F({ category: 'request-failed', detail: `GET ${url} — net::ERR_ABORTED` })

test('TEST-10 a dyn-import crash IS muted when the SAME module failed in that cell', () => {
  const c = crash(DEV_SRC)
  assert.equal(classifyIn(c, [c, abortedRequest(DEV_SRC)]), true)
})

test('TEST-10b the same crash still GATES with NO corroborating request', () => {
  const c = crash(DEV_SRC)
  assert.equal(
    classifyIn(c, [c]),
    false,
    'without evidence the module failed at the transport layer, this is a product crash',
  )
})

test('TEST-10c a crash is NOT muted by a DIFFERENT module\'s routine abort', () => {
  // The finding a reviewer raised: a dev-asset ERR_ABORTED is documented ROUTINE
  // noise (36 in the origin/main baseline). If ANY abort corroborated, a
  // genuinely broken lazy() import would be muted whenever one happened to occur
  // in the same cell, and the surface would report PASS while rendering only an
  // ErrorBoundary.
  const c = crash('http://localhost/src/modules/broken/Really.tsx')
  assert.equal(classifyIn(c, [c, abortedRequest(DEV)]), false)
})

test('TEST-10d corroboration ignores the query string (same module, different ?t=)', () => {
  const c = crash('http://localhost/src/a/B.tsx?t=999')
  assert.equal(classifyIn(c, [c, abortedRequest('http://localhost/src/a/B.tsx?t=111')]), true)
})

test('TEST-10e an ErrorBoundary crash with ANY other message is NEVER muted', () => {
  const c = F({ category: 'crash', detail: '[AppErrorBoundary] TypeError: x is not a function' })
  assert.equal(classifyIn(c, [c, abortedRequest(DEV)]), false)
})

test('TEST-10f a dyn-import crash for a PRODUCT url is never muted', () => {
  const c = crash(PRODUCT)
  assert.equal(classifyIn(c, [c, abortedRequest(PRODUCT)]), false)
})

// ---------------------------------------------------------------------------
// Corroboration must be built from the extracted URL, not the whole detail text.
// ---------------------------------------------------------------------------
test('a PRODUCT failure whose TEXT merely contains a dev-asset-shaped substring does not corroborate', () => {
  const productWithDecoyText = F({
    category: 'request-failed',
    detail: 'GET http://localhost/api/x?next=/src/foo.tsx — net::ERR_FAILED',
  })
  const c = crash('http://localhost/src/foo.tsx')
  assert.equal(
    classifyIn(c, [c, productWithDecoyText]),
    false,
    'the corroboration signal must not be looser than the mute it authorises',
  )
  assert.equal(
    classifyIn(productWithDecoyText, [c, productWithDecoyText]),
    false,
    'and that product failure itself still gates',
  )
})

// ---------------------------------------------------------------------------
// TEST-11 — classifyAll wiring.
// ---------------------------------------------------------------------------
test('TEST-11 classifyAll stamps harness on HIGH only, and baselined wins', () => {
  const findings = [
    F({ detail: 'Failed to load resource: net::ERR_ABORTED', resourceUrl: DEV }),
    F({ severity: 'LOW', category: 'spacing-grid', detail: 'padding 7px' }),
    F({ detail: 'TypeError: real product bug' }),
    F({ detail: 'baseline item' }),
  ]
  classifyAll(findings, f => f.detail === 'baseline item')
  assert.equal(findings[0].harness, true, 'muted transport mirror')
  assert.equal(findings[1].harness, undefined, 'LOW is never classified')
  assert.equal(findings[2].harness, undefined, 'a real error is untouched')
  assert.equal(findings[3].baselined, true)
  assert.equal(findings[3].harness, undefined, 'baselined takes precedence')
})

test('TEST-11b evidence is per CELL — another cell\'s abort does not corroborate', () => {
  const c = crash(DEV_SRC)
  const otherCell = F({
    surface: 'OTHER',
    category: 'request-failed',
    detail: `GET ${DEV_SRC} — net::ERR_ABORTED`,
  })
  assert.equal(classifyIn(c, [c, otherCell]), false)
})

// ---------------------------------------------------------------------------
// Helper units.
// ---------------------------------------------------------------------------
test('isViteDevAsset distinguishes dev assets from product endpoints', () => {
  for (const u of [DEV, DEV_SRC, 'http://x/@vite/client', 'http://x/node_modules/a/b.js'])
    assert.equal(isViteDevAsset(u), true, u)
  for (const u of [PRODUCT, 'http://x/api/files/1', 'http://x/'])
    assert.equal(isViteDevAsset(u), false, u)
})

test('requestUrlOf extracts the url from a request-failed detail', () => {
  assert.equal(requestUrlOf('GET http://x/a.js — net::ERR_ABORTED'), 'http://x/a.js')
  assert.equal(requestUrlOf('no url here'), null)
})

test('normalizeAssetUrl strips the query', () => {
  assert.equal(normalizeAssetUrl('http://x/src/a.tsx?t=1'), '/src/a.tsx')
  assert.equal(normalizeAssetUrl('/src/a.tsx?import'), '/src/a.tsx')
})

// ---------------------------------------------------------------------------
// TEST-32 — console-message classification is CHANNEL-INDEPENDENT.
//
// React 19 routes its developer warnings through `console.error`, not
// `console.warn`. The harness taxonomy classifies `unique "key" prop` (and the
// rest of REACT_WARNING) as `react-warning`/MEDIUM, but that branch used to be
// consulted ONLY when `msg.type() === 'warning'` — so on React 19 every React
// warning was recorded as `console-error`/HIGH and GATED the surface. That is a
// misclassification against the harness's own taxonomy, not a flake.
//
// Asserted in BOTH directions: the warning texts must be MEDIUM on the error
// channel, AND a genuine runtime error on the same channel must still be HIGH
// (a blanket downgrade of console.error would blind the gate entirely).
// ---------------------------------------------------------------------------
test('TEST-32 a React warning on the ERROR channel is react-warning/MEDIUM', () => {
  for (const t of [
    'Each child in a list should have a unique "key" prop.',
    'Warning: validateDOMNesting: <div> cannot appear as a descendant of <p>.',
    'An update to Foo inside a test was not wrapped in act(...)',
  ])
    assert.deepEqual(
      classifyConsoleMessage('error', 'loaded', t),
      { category: 'react-warning', severity: 'MEDIUM' },
      t,
    )
})

test('TEST-32 the same texts on the WARNING channel are unchanged (MEDIUM)', () => {
  assert.deepEqual(
    classifyConsoleMessage('warning', 'loaded', 'Each child in a list should have a unique "key" prop.'),
    { category: 'react-warning', severity: 'MEDIUM' },
  )
})

test('TEST-32 a genuine console error still gates HIGH — no blanket downgrade', () => {
  for (const t of [
    'TypeError: Cannot read properties of undefined (reading "map")',
    'Internal React error: Expected static flag was missing. Please notify the React team.',
    'Failed to fetch /api/files',
  ])
    assert.deepEqual(
      classifyConsoleMessage('error', 'loaded', t),
      { category: 'console-error', severity: 'HIGH' },
      t,
    )
})

test('TEST-32 an ErrorBoundary crash outranks the warning arm and stays HIGH', () => {
  // The fixture MUST match a react-warning pattern too, or the two arms never
  // contend and the test passes with the precedence reversed. (An ErrorBoundary
  // report quotes the error it caught, so this text shape is realistic.)
  for (const t of [
    '[AppErrorBoundary [x]] Error: Each child in a list should have a unique "key" prop.',
    '[AppErrorBoundary [x]] Error: validateDOMNesting failed',
  ]) {
    assert.ok(
      REACT_WARNING_STRICT.some(re => re.test(t)),
      `fixture must actually match the warning arm, else the test is hollow: ${t}`,
    )
    assert.deepEqual(classifyConsoleMessage('error', 'loaded', t), {
      category: 'crash',
      severity: 'HIGH',
    })
  }
})

test('TEST-32 a NON-React error containing warning-list words still gates HIGH', () => {
  // Both blind auditors found this independently: the historical react-warning
  // list is deliberately loose, which is harmless on the warning channel (already
  // non-gating) but is a GATE HOLE on the error channel. These are real product
  // failures whose text merely contains an English phrase from that list.
  for (const t of [
    '[ApiClient] GET /api/models failed: 410 Gone — this endpoint is deprecated',
    'Uncaught (in promise) Error: crypto.subtle is deprecated in insecure contexts',
    'TypeError: ReactDOM.findDOMNode is not a function',
    'Warning: Failed to persist draft — data loss',
  ])
    assert.deepEqual(
      classifyConsoleMessage('error', 'loaded', t),
      { category: 'console-error', severity: 'HIGH' },
      `must still gate: ${t}`,
    )
})

test('TEST-32 the WARNING channel keeps the loose list (unchanged behaviour)', () => {
  // The loose patterns are still honoured where they cost nothing.
  for (const t of [
    'Warning: something reactish',
    'ReactDOM.render is deprecated and will be removed',
    'findDOMNode is deprecated in StrictMode',
  ])
    assert.deepEqual(
      classifyConsoleMessage('warning', 'loaded', t),
      { category: 'react-warning', severity: 'MEDIUM' },
      t,
    )
})

test('TEST-32 non-diagnostic channels record NOTHING (no ledger inflation)', () => {
  // Consulting the warning list before the channel would newly record ordinary
  // app logging — including, in the gating direction, an ErrorBoundary log.
  for (const ch of ['log', 'info', 'debug', 'trace'])
    for (const t of [
      'ReactDOM.render is deprecated',
      'Each child in a list should have a unique "key" prop.',
      '[AppErrorBoundary [x]] Error: boom',
    ])
      assert.equal(classifyConsoleMessage(ch, 'loaded', t), null, `${ch}: ${t}`)
})

test('TEST-32 the error-state MEDIUM downgrade is preserved for real errors', () => {
  assert.deepEqual(
    classifyConsoleMessage('error', 'error', 'TypeError: boom'),
    { category: 'console-error', severity: 'MEDIUM' },
  )
})

test('TEST-32 a non-React message on the warning channel is not recorded', () => {
  assert.equal(classifyConsoleMessage('warning', 'loaded', 'some random warning'), null)
})

// ---------------------------------------------------------------------------
// F6 — each STRICT pattern is pinned INDIVIDUALLY.
//
// The earlier fixtures each matched two or more patterns and asserted only on
// the RESULT, so deleting `/unique "key" prop/i` or `/Each child in a list/i`
// was a silent no-op (they mask each other). A pattern nobody pins can be
// deleted or rot against a React upgrade without a test noticing — which is
// exactly what happened to the DOM-nesting patterns.
//
// One case per pattern, each text matching THAT pattern and no other.
// ---------------------------------------------------------------------------
const STRICT_CASES = [
  ['key warning', 'Each child in a list should have a unique "key" prop.'],
  ['not wrapped in act(', 'An update to Foo inside a test was not wrapped in act(...)'],
  ['React 19 DOM nesting', 'In HTML, <div> cannot be a child of <p>. This will cause a hydration error.'],
  ['React 19 DOM nesting (descendant)', 'In HTML, <div> cannot be a descendant of <p>. This will cause a hydration error.'],
  ['validateDOMNesting (React 18)', 'validateDOMNesting: something is off'],
  ['cannot appear as a descendant of (React 18)', '<td> cannot appear as a descendant of <div>.'],
]

test('TEST-32 each STRICT pattern is individually load-bearing', () => {
  for (const [name, text] of STRICT_CASES) {
    const matching = REACT_WARNING_STRICT.filter(re => re.test(text))
    assert.equal(
      matching.length,
      1,
      `"${name}" must be pinned by EXACTLY ONE pattern or deleting that pattern ` +
        `is masked by another; matched ${matching.length}: ${matching.map(String).join(', ')}`,
    )
    assert.deepEqual(
      classifyConsoleMessage('error', 'loaded', text),
      { category: 'react-warning', severity: 'MEDIUM' },
      `${name}: must be downgraded on the error channel`,
    )
  }
})

// NOTE: the "does the STRICT list match the INSTALLED React?" backstop lives in
// the CONSUMER test (`src-app/ui/scripts/react-warning-coverage.test.mjs`).
// react-dom is the APP's dependency, not this package's, so resolving it from
// here silently found nothing and the test skipped itself into a vacuous pass —
// which let a wrong pattern spelling go green. A test that cannot see its
// subject must live where its subject is.
