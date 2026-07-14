/**
 * PART 1 — STATIC STATE EXTRACTION (the checklist generator).
 *
 * An AST pass (ts-morph) over every component/page under `src/modules` +
 * `src/components/ui` that mechanically extracts the RENDERABLE STATES a surface
 * can be in — so the required-state matrix comes from the code, not from an
 * agent's judgment (the gap the seeded gallery had: it hand-listed states and
 * missed empty/error and the ConversationPage deep states).
 *
 * Per surface it extracts:
 *   (a) every CONDITIONAL RENDER + its governing condition — `if (c) return …`,
 *       `c ? <A> : <B>`, `c && <JSX>` — classified loading / error / empty /
 *       branch by the identifiers in the condition;
 *   (b) every OVERLAY open-trigger — a Dialog/Drawer/Sheet/Popover/Dropdown/
 *       Modal/Confirm/AlertDialog element with a controlled `open`/`visible` prop;
 *   (c) every PANEL RENDERER + SLOT registration — `registerPanelRenderer('x')`
 *       and the module-system slot keys (sidebarContent, settings*Pages, the chat
 *       right-panel, header-trailing …) — this is HOW the right-panel file viewer /
 *       literature panel / elicitation UI become discoverable.
 *
 * Emits a GENERATED matrix + a TSC-ENFORCED state-granularity coverage union
 * (mirrors galleryCoverage.generated.ts's surface union → coverage.ts Record):
 *   - src/dev/gallery/stateMatrix.generated.ts   (typed matrix + `RequiredState`
 *                                                 = "surface:state" union — one
 *                                                 member per named required state)
 *   - src/dev/gallery/STATE_MATRIX.md             (human-readable review artifact)
 *
 * The hand-maintained src/dev/gallery/stateCoverage.ts then declares
 *   STATE_COVERAGE satisfies Record<RequiredState, StateCoverageEntry>
 * so a NEWLY-EXTRACTED required state with no entry is a COMPILE ERROR, and every
 * deliberate gap is excused in code with a reason (`{ skip: true, reason }`).
 * `--check` is the byte-parity guard (like the openapi types_ts_parity test):
 * a new conditional render makes the generated file stale → check fails → regen
 * adds a `RequiredState` member → tsc fails on stateCoverage.ts until it gets a
 * gallery entry or an allow-listed reason.
 *
 * Run: node scripts/gen-state-matrix.mjs            (write matrix + union)
 *      node scripts/gen-state-matrix.mjs --check    (drift guard — fail if stale)
 *      node scripts/gen-state-matrix.mjs --scaffold (append missing RequiredState
 *                                                    keys to stateCoverage.ts as
 *                                                    allow-listed gaps to review)
 */
import path from 'node:path'
import fs from 'node:fs'
import { Project, SyntaxKind, Node } from 'ts-morph'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

// Config-driven anchors (was HERE/UI_DIR/SRC + hardcoded gallery-dir outputs).
// The app runs this with its `ui/` as cwd, so `gallery.config.json` + the
// relative paths resolve against it.
const CFG = resolveGalleryConfig()
const UI_DIR = CFG.__cwd
const SRC = path.resolve(UI_DIR, CFG.srcDir)
const GALLERY_DIR = path.resolve(UI_DIR, CFG.galleryDir)
const OUT_TS = path.join(GALLERY_DIR, 'stateMatrix.generated.ts')
const OUT_MD = path.join(GALLERY_DIR, 'STATE_MATRIX.md')
const OUT_COVERAGE = path.join(GALLERY_DIR, 'stateCoverage.ts')

// The same roots the coverage denominator walks (gen-gallery-coverage.mjs), so a
// matrix surface id == a coverage surface id (path from src/ without extension).
const GLOBS = CFG.surfaceRoots.map(r => path.join(path.resolve(UI_DIR, r), '**/*.tsx'))

const surfaceId = absFile =>
  path.relative(SRC, absFile).replace(/\\/g, '/').replace(/\.tsx$/, '')

// ── Condition → state kind classification ────────────────────────────────────
// A render-gating condition's identifiers place it in a state bucket. Order
// matters: error before loading before empty (an `!loading && error` reads as
// error). `branch` is the residual — a conditional render we can't bucket but
// that still forks the output (tracked, not gated).
const CLASSIFIERS = [
  ['error', /\b(error|isError|hasError|failed|loadError|fetchError)\b/i],
  ['loading', /\b(loading|isLoading|isPending|isFetching|isInitializing|isLoading|pending|spinner|skeleton)\b/i],
  ['empty', /\.length\s*===\s*0|\.length\s*<\s*1|===\s*0\b|\bisEmpty\b|\bempty\b|!\w+\.length|\bno[A-Z]\w+\b|\.size\s*===\s*0|length\s*===\s*0/],
]
function classify(conditionText) {
  for (const [kind, rx] of CLASSIFIERS) if (rx.test(conditionText)) return kind
  return 'branch'
}

const OVERLAY_TAG = /(AlertDialog|Dialog|Drawer|Sheet|Popover|DropdownMenu|Dropdown|Modal|Confirm|HoverCard|ContextMenu)$/
const OVERLAY_OPEN_PROP = /^(open|visible|defaultOpen|isOpen)$/

// Module-system slot keys — a `registerSlot`/module `slots` registration into one
// of these makes a renderer discoverable at that mount point. Extends over time;
// unknown keys are still captured generically via `registerSlot(`.
const KNOWN_SLOTS = new Set([
  'sidebarContent',
  'settingsAdminPages',
  'settingsUserPages',
  'chatConversationHeaderTrailing',
  'chatInputActions',
  'chatMessageActions',
  'headerRight',
  'userMenuItems',
  'knowledgeKinds',
  'projectExtensions',
])

/** Does this expression subtree contain JSX (i.e. the branch RENDERS)? */
function containsJsx(node) {
  if (!node) return false
  if (
    Node.isJsxElement(node) ||
    Node.isJsxSelfClosingElement(node) ||
    Node.isJsxFragment(node)
  )
    return true
  // A `null`/`undefined`/`false` literal counts as a render branch (the "hide"
  // arm of a conditional render) only when its sibling arm renders — handled by
  // the caller, which only records when the OTHER side is JSX.
  return node.getDescendantsOfKind(SyntaxKind.JsxElement).length > 0 ||
    node.getDescendantsOfKind(SyntaxKind.JsxSelfClosingElement).length > 0 ||
    node.getDescendantsOfKind(SyntaxKind.JsxFragment).length > 0
}

const isNullish = node =>
  !node ||
  node.getKind() === SyntaxKind.NullKeyword ||
  (Node.isIdentifier(node) && node.getText() === 'undefined') ||
  (Node.isLiteralExpression(node) && node.getText() === 'false') ||
  node.getText() === 'false'

const clean = s => s.replace(/\s+/g, ' ').trim().slice(0, 160)

function extractSurface(sourceFile) {
  const signals = []
  const seen = new Set()
  const push = (kind, condition, node) => {
    const line = node.getStartLineNumber()
    const key = `${kind}::${clean(condition)}::${line}`
    if (seen.has(key)) return
    seen.add(key)
    signals.push({ kind, condition: clean(condition), line })
  }

  // (a) conditional renders ---------------------------------------------------
  // Ternaries whose branch(es) render JSX.
  for (const cond of sourceFile.getDescendantsOfKind(SyntaxKind.ConditionalExpression)) {
    const whenTrue = cond.getWhenTrue()
    const whenFalse = cond.getWhenFalse()
    const trueRenders = containsJsx(whenTrue)
    const falseRenders = containsJsx(whenFalse)
    if (!trueRenders && !falseRenders) continue
    const c = cond.getCondition().getText()
    // A `cond ? <JSX> : null` gates the true-branch on `cond`; a
    // `cond ? null : <JSX>` gates the visible branch on `!cond`.
    if (trueRenders && isNullish(whenFalse)) push(classify(c), c, cond)
    else if (falseRenders && isNullish(whenTrue)) push(classify(`!(${c})`), `!(${c})`, cond)
    else push(classify(c), c, cond) // both render (variant fork)
  }

  // Logical `cond && <JSX>`.
  for (const bin of sourceFile.getDescendantsOfKind(SyntaxKind.BinaryExpression)) {
    if (bin.getOperatorToken().getKind() !== SyntaxKind.AmpersandAmpersandToken) continue
    const right = bin.getRight()
    if (!containsJsx(right)) continue
    const c = bin.getLeft().getText()
    push(classify(c), c, bin)
  }

  // Early `if (cond) return <JSX|null>` — the guard-clause render pattern
  // (ConversationPage's loading / not-found returns are exactly this).
  for (const ifStmt of sourceFile.getDescendantsOfKind(SyntaxKind.IfStatement)) {
    const thenRet = firstReturn(ifStmt.getThenStatement())
    if (!thenRet) continue
    const retExpr = thenRet.getExpression()
    const renders = containsJsx(retExpr) || isNullish(retExpr)
    if (!renders) continue
    // Only count guard clauses that render JSX in EITHER the then-return or the
    // function's fallthrough (a `return null` guard hides content = a state).
    if (!containsJsx(retExpr) && !isNullish(retExpr)) continue
    const c = ifStmt.getExpression().getText()
    push(classify(c), c, ifStmt)
  }

  // (b) overlay open-triggers -------------------------------------------------
  for (const kind of [SyntaxKind.JsxOpeningElement, SyntaxKind.JsxSelfClosingElement]) {
    for (const el of sourceFile.getDescendantsOfKind(kind)) {
      const tag = el.getTagNameNode?.().getText?.() ?? ''
      if (!OVERLAY_TAG.test(tag)) continue
      const hasOpenProp = el
        .getAttributes()
        .some(a => Node.isJsxAttribute(a) && OVERLAY_OPEN_PROP.test(a.getNameNode().getText()))
      if (hasOpenProp) push('overlay', `<${tag} open>`, el)
    }
  }

  // (c) panel renderers + slot registrations ----------------------------------
  const panels = []
  const slots = []
  for (const call of sourceFile.getDescendantsOfKind(SyntaxKind.CallExpression)) {
    const callee = call.getExpression().getText()
    if (/(^|\.)registerPanelRenderer$/.test(callee)) {
      const arg0 = call.getArguments()[0]
      if (arg0 && Node.isStringLiteral(arg0)) {
        panels.push({ type: arg0.getLiteralText(), line: call.getStartLineNumber() })
        push('panel', `registerPanelRenderer('${arg0.getLiteralText()}')`, call)
      }
    }
    if (/(^|\.)registerSlot$/.test(callee)) {
      const arg0 = call.getArguments()[0]
      if (arg0 && Node.isStringLiteral(arg0)) {
        slots.push({ slot: arg0.getLiteralText(), line: call.getStartLineNumber() })
      }
    }
  }
  // Object-literal slot registrations: `slots: { sidebarContent: [...] }` in a
  // module.tsx export.
  for (const pa of sourceFile.getDescendantsOfKind(SyntaxKind.PropertyAssignment)) {
    const name = pa.getNameNode().getText().replace(/['"]/g, '')
    if (KNOWN_SLOTS.has(name)) slots.push({ slot: name, line: pa.getStartLineNumber() })
  }

  return { signals, panels, slots }
}

function firstReturn(stmt) {
  if (!stmt) return undefined
  if (Node.isReturnStatement(stmt)) return stmt
  if (Node.isBlock(stmt)) {
    for (const s of stmt.getStatements()) if (Node.isReturnStatement(s)) return s
  }
  return undefined
}

// Signal kind → the gallery GalleryState it demands.
const KIND_TO_STATE = {
  loading: 'delayed',
  error: 'error',
  empty: 'empty',
  overlay: 'open',
  panel: 'panel-open',
  // `branch` demands no specific gallery state (it's a variant fork; the dynamic
  // branch-coverage pass is what proves a generic branch actually rendered).
}

function deriveRequiredStates(signals) {
  const states = new Set()
  for (const s of signals) {
    const st = KIND_TO_STATE[s.kind]
    if (st) states.add(st)
  }
  return [...states].sort()
}

// ── Build the matrix ─────────────────────────────────────────────────────────
const project = new Project({
  tsConfigFilePath: path.resolve(UI_DIR, CFG.tsconfig),
  skipAddingFilesFromTsConfig: true,
})
const files = []
for (const glob of GLOBS) files.push(...project.addSourceFilesAtPaths(glob))
// module.tsx is a registration manifest, not a visual surface — but we DO want
// its slot registrations. Handle both: extract slots from every file; only emit
// a surface matrix row for non-module.tsx files (mirrors the coverage walker).

const matrix = {}
const allPanels = []
const allSlots = []
let signalCount = 0
for (const sf of files) {
  const abs = sf.getFilePath()
  const base = path.basename(abs)
  // `.desktop.tsx` co-located overrides are desktop-only (excluded from the web
  // tsconfig/biome + the coverage walker) — not web surfaces. Per-module
  // `gallery.tsx` seed files are authoring metadata, not surfaces either.
  if (/\.desktop\.tsx$/.test(base) || base === 'gallery.tsx' || base === 'gallery.ts')
    continue
  const { signals, panels, slots } = extractSurface(sf)
  const id = surfaceId(abs)
  for (const p of panels) allPanels.push({ ...p, surface: id })
  for (const s of slots) allSlots.push({ ...s, surface: id })
  if (base === 'module.tsx') continue // not a visual surface (registration only)
  if (!signals.length) continue // no renderable-state signal → nothing to require
  signalCount += signals.length
  matrix[id] = {
    surface: id,
    signals: signals.sort((a, b) => a.line - b.line),
    requiredStates: deriveRequiredStates(signals),
  }
}

const sortedIds = Object.keys(matrix).sort()
allPanels.sort((a, b) => a.type.localeCompare(b.type))
allSlots.sort((a, b) => a.slot.localeCompare(b.slot) || a.surface.localeCompare(b.surface))

// The `RequiredState` union — "surface:state" for every NAMED required state
// (loading/error/empty/overlay/panel → delayed/error/empty/open/panel-open).
// Generic `branch` signals get no key here — they are proven by Part 2's runtime
// branch coverage, not the tsc gate.
const requiredStateKeys = []
for (const id of sortedIds)
  for (const st of matrix[id].requiredStates) requiredStateKeys.push(`${id}:${st}`)
requiredStateKeys.sort()

// ── Render stateMatrix.generated.ts ──────────────────────────────────────────
function renderTs() {
  const rows = sortedIds
    .map(id => {
      const m = matrix[id]
      const sig = m.signals
        .map(s => `      { kind: ${JSON.stringify(s.kind)}, condition: ${JSON.stringify(s.condition)}, line: ${s.line} },`)
        .join('\n')
      return `  ${JSON.stringify(id)}: {
    surface: ${JSON.stringify(id)},
    requiredStates: ${JSON.stringify(m.requiredStates)},
    signals: [
${sig}
    ],
  },`
    })
    .join('\n')
  const panels = allPanels
    .map(p => `  { type: ${JSON.stringify(p.type)}, surface: ${JSON.stringify(p.surface)}, line: ${p.line} },`)
    .join('\n')
  const slots = allSlots
    .map(s => `  { slot: ${JSON.stringify(s.slot)}, surface: ${JSON.stringify(s.surface)}, line: ${s.line} },`)
    .join('\n')
  return `// AUTO-GENERATED by scripts/gen-state-matrix.mjs — DO NOT EDIT.
// Run \`npm run gen:state-matrix\` to refresh. This is PART 1 of the exhaustive-
// state mechanism: the mechanically-extracted required-state matrix (conditional
// renders + overlay triggers + panel/slot registrations) that the reconciliation
// gate (scripts/reconcile-state-matrix.mjs) checks the gallery entries against.
//
// ${sortedIds.length} surfaces carry renderable-state signals; ${signalCount} signals total.

/** A signal is one mechanically-detected render fork (a state the surface can be in). */
export interface StateSignal {
  kind: 'loading' | 'error' | 'empty' | 'branch' | 'overlay' | 'panel'
  /** The governing condition text (or the overlay tag / registration). */
  condition: string
  line: number
}

export interface SurfaceStateMatrix {
  surface: string
  /** Gallery states this surface's signals demand (delayed/error/empty/open/panel-open). */
  requiredStates: string[]
  signals: StateSignal[]
}

/** Every panel renderer registered via \`registerPanelRenderer('type', …)\`. */
export interface PanelRegistration {
  type: string
  surface: string
  line: number
}

/** Every module-system slot registration (sidebarContent, settings*Pages, …). */
export interface SlotRegistration {
  slot: string
  surface: string
  line: number
}

export const STATE_MATRIX: Record<string, SurfaceStateMatrix> = {
${rows}
}

/** Right-panel renderers — each is a distinct right-panel-open state to render. */
export const PANEL_RENDERERS: PanelRegistration[] = [
${panels}
]

/** Slot registrations (discoverability map for sidebar/settings/panel mount points). */
export const SLOT_REGISTRATIONS: SlotRegistration[] = [
${slots}
]

export type StateMatrixSurface = keyof typeof STATE_MATRIX

/**
 * The TSC-ENFORCED coverage key set: one \`"<surface>:<state>"\` per named
 * required state extracted above. \`stateCoverage.ts\` declares
 * \`STATE_COVERAGE satisfies Record<RequiredState, StateCoverageEntry>\`, so a
 * newly-extracted state with no entry is a compile error (mirrors how
 * galleryCoverage.generated.ts's \`GallerySurface\` gates coverage.ts).
 * ${requiredStateKeys.length} keys.
 */
export type RequiredState =
${requiredStateKeys.map(k => `  | ${JSON.stringify(k)}`).join('\n')}

/** Every required-state key, at runtime (for the scaffold + reporting). */
export const REQUIRED_STATE_KEYS = [
${requiredStateKeys.map(k => `  ${JSON.stringify(k)},`).join('\n')}
] as const
`
}

// ── Render STATE_MATRIX.md ───────────────────────────────────────────────────
function renderMd() {
  const byState = {}
  for (const id of sortedIds)
    for (const s of matrix[id].requiredStates) (byState[s] ??= []).push(id)
  const kindTotals = {}
  for (const id of sortedIds)
    for (const sig of matrix[id].signals) kindTotals[sig.kind] = (kindTotals[sig.kind] ?? 0) + 1

  let md = `# Required-state matrix (GENERATED)

> Auto-generated by \`scripts/gen-state-matrix.mjs\` — do not edit by hand.
> PART 1 of the gallery exhaustive-state mechanism: the states each surface can
> render, extracted mechanically from the AST (conditional renders, overlay
> open-triggers, panel/slot registrations) rather than from agent judgment.

## Summary

- **${sortedIds.length}** surfaces carry at least one renderable-state signal.
- **${signalCount}** signals total: ${Object.entries(kindTotals).sort().map(([k, n]) => `${n} ${k}`).join(', ')}.
- **${allPanels.length}** right-panel renderers registered (each a right-panel-open state).
- **${allSlots.length}** slot registrations (sidebar / settings / chat mount points).

### Surfaces demanding each gallery state

| state | surfaces |
|---|---|
${Object.entries(byState).sort().map(([st, ids]) => `| \`${st}\` | ${ids.length} |`).join('\n')}

## Right-panel renderers (\`registerPanelRenderer\`)

These are the discoverable right-panel deep states (file viewer, literature,
tool-result, …) — the states the seeded gallery missed on the active
conversation page.

| panel type | registered in |
|---|---|
${allPanels.map(p => `| \`${p.type}\` | \`${p.surface}\`:${p.line} |`).join('\n') || '| _(none)_ | |'}

## Slot registrations

| slot | module surface |
|---|---|
${allSlots.map(s => `| \`${s.slot}\` | \`${s.surface}\`:${s.line} |`).join('\n') || '| _(none)_ | |'}

## Per-surface required states

`
  for (const id of sortedIds) {
    const m = matrix[id]
    md += `### \`${id}\`\n\n`
    md += `Required states: ${m.requiredStates.length ? m.requiredStates.map(s => `\`${s}\``).join(', ') : '_(branch-only — proven via dynamic coverage)_'}\n\n`
    md += `| kind | condition | line |\n|---|---|---|\n`
    for (const s of m.signals) md += `| ${s.kind} | \`${s.condition.replace(/\|/g, '\\|')}\` | ${s.line} |\n`
    md += '\n'
  }
  return md
}

// ── stateCoverage.ts scaffolding ─────────────────────────────────────────────
// The hand-maintained Record is authored by humans; --scaffold only APPENDS
// missing keys as allow-listed gaps (like gen-gallery-coverage --scaffold), so
// the tsc gate keeps firing on genuinely new states.
const COVERAGE_MARKER = '  // <<< state-scaffold-insert >>>'
let SURFACE_KINDS = {}

function parseCoverageKeys() {
  if (!fs.existsSync(OUT_COVERAGE)) return new Set()
  const src = fs.readFileSync(OUT_COVERAGE, 'utf8')
  const keys = new Set()
  const re = /['"]([^'"]+:[^'"]+)['"]:\s*\{/g
  let m
  while ((m = re.exec(src))) keys.add(m[1])
  return keys
}

// Surface → kind, read from the surface-level coverage.ts, so the scaffold emits
// a SEMANTICALLY-CORRECT default per key instead of a blanket skip:
//   data-page/table + error|empty|delayed → delivered via the page state-mode;
//   overlay + open                        → delivered via an overlay entry;
//   everything else                       → an allow-listed gap (rendered inside
//                                            its page; branch proven by Part 2).
function parseSurfaceKinds() {
  const p = path.join(GALLERY_DIR, 'coverage.ts')
  const out = {}
  if (!fs.existsSync(p)) return out
  const src = fs.readFileSync(p, 'utf8')
  const re = /"([^"]+)":\s*\{\s*kind:\s*'([^']+)'/g
  let m
  while ((m = re.exec(src))) out[m[1]] = m[2]
  return out
}

function scaffoldEntryFor(key) {
  const idx = key.lastIndexOf(':')
  const surface = key.slice(0, idx)
  const state = key.slice(idx + 1)
  const kind = SURFACE_KINDS[surface]
  if ((kind === 'data-page' || kind === 'table') && ['error', 'empty', 'delayed'].includes(state))
    return `{ via: 'page-state-mode' }`
  if (kind === 'overlay' && state === 'open') return `{ via: 'overlay' }`
  const why =
    kind && kind !== 'pending'
      ? `${kind} surface — rendered within its page; '${state}' branch proven by Part 2 runtime coverage`
      : `rendered within its parent page; '${state}' branch proven by Part 2 runtime coverage`
  return `{ skip: true, reason: ${JSON.stringify(why)} }`
}

// ── Write / check / scaffold ─────────────────────────────────────────────────
const tsBody = renderTs()
const mdBody = renderMd()
const mode = process.argv.includes('--check')
  ? 'check'
  : process.argv.includes('--scaffold')
    ? 'scaffold'
    : 'write'

if (mode === 'check') {
  const curTs = fs.existsSync(OUT_TS) ? fs.readFileSync(OUT_TS, 'utf8') : ''
  const curMd = fs.existsSync(OUT_MD) ? fs.readFileSync(OUT_MD, 'utf8') : ''
  const stale = curTs.trim() !== tsBody.trim() || curMd.trim() !== mdBody.trim()
  if (stale) {
    console.error(
      'state matrix is stale — run `npm run gen:state-matrix` and commit (a new\n' +
        'conditional render was added). Regen adds its RequiredState key; then map\n' +
        'it in stateCoverage.ts (a gallery entry or an allow-listed reason).',
    )
    process.exit(1)
  }
  // Freshness of the coverage Record's key set vs the generated union is enforced
  // by tsc (satisfies Record<RequiredState, …>); here we just report residuals.
  const covered = parseCoverageKeys()
  const missing = requiredStateKeys.filter(k => !covered.has(k))
  console.log(
    `state matrix up to date (${sortedIds.length} surfaces, ${signalCount} signals, ` +
      `${requiredStateKeys.length} required-state keys, ${allPanels.length} panels).`,
  )
  if (missing.length) {
    console.error(
      `\n${missing.length} required-state key(s) not yet in stateCoverage.ts — run ` +
        `\`node scripts/gen-state-matrix.mjs --scaffold\` (tsc also fails on these):`,
    )
    for (const k of missing.slice(0, 20)) console.error(`  ✗ ${k}`)
    process.exit(1)
  }
} else if (mode === 'scaffold') {
  fs.writeFileSync(OUT_TS, tsBody)
  fs.writeFileSync(OUT_MD, mdBody)
  if (!fs.existsSync(OUT_COVERAGE)) {
    console.error(`stateCoverage.ts does not exist yet — create it first (see the header).`)
    process.exit(1)
  }
  const cov = fs.readFileSync(OUT_COVERAGE, 'utf8')
  if (!cov.includes(COVERAGE_MARKER)) {
    console.error(`stateCoverage.ts is missing the insert marker "${COVERAGE_MARKER}".`)
    process.exit(1)
  }
  SURFACE_KINDS = parseSurfaceKinds()
  const have = parseCoverageKeys()
  const missing = requiredStateKeys.filter(k => !have.has(k))
  if (!missing.length) {
    console.log('stateCoverage.ts already maps every required state.')
  } else {
    const inserts = missing
      .map(k => `  ${JSON.stringify(k)}: ${scaffoldEntryFor(k)},`)
      .join('\n')
    fs.writeFileSync(OUT_COVERAGE, cov.replace(COVERAGE_MARKER, `${inserts}\n${COVERAGE_MARKER}`))
    console.log(`Added ${missing.length} allow-listed gap(s) to stateCoverage.ts (review + upgrade).`)
  }
} else {
  fs.writeFileSync(OUT_TS, tsBody)
  fs.writeFileSync(OUT_MD, mdBody)
  console.log(
    `Wrote ${path.relative(UI_DIR, OUT_TS)} + ${path.relative(UI_DIR, OUT_MD)} ` +
      `(${sortedIds.length} surfaces, ${signalCount} signals, ${requiredStateKeys.length} required-state keys).`,
  )
}
