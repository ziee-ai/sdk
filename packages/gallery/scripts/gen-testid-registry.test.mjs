/**
 * TEST — the testid-registry generator's pure core.
 *
 * The generator used to be a TEXT SCAN (`/data-testid\s*[=:]\s*["']([^"']+)["']/g`)
 * for the attribute followed by a quoted value, so it harvested any quoted string
 * in that shape — including out of comments and out of `querySelector` template
 * strings — and it simultaneously MISSED every id written in a `??`/ternary value
 * position. Both directions are covered below; the fixture-based tests are what
 * make comment/interpolation immunity a STRUCTURAL property rather than a pattern
 * that the next unusual spelling escapes.
 *
 * Run: node --test scripts/gen-testid-registry.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  ID_SHAPE,
  assertIdShapes,
  collectSourceFiles,
  collectTestIdSites,
  collectTestIds,
  renderRegistry,
} from './gen-testid-registry.mjs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))

/** Write `src` to a temp `.tsx` and return its path + a cleanup fn. */
function fixture(src, ext = 'tsx') {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  const file = path.join(dir, `Fixture.${ext}`)
  fs.writeFileSync(file, src)
  return { file, cleanup: () => fs.rmSync(dir, { recursive: true, force: true }) }
}

// ---------------------------------------------------------------------------
// TEST-5 (acceptance, INV-5) — comments and interpolations are NOT harvestable,
// and the negative control in the SAME fixture proves real ids still are.
// ---------------------------------------------------------------------------
test('TEST-5 [acceptance INV-5] comments + interpolations yield nothing; real attributes still do', () => {
  const CONST = 'x'
  void CONST
  const { file, cleanup } = fixture(`
// (a) a plain line comment that names data-testid="phantom-from-line-comment"
/**
 * (b) a JSDoc block. Optional unique data-testid="phantom-from-jsdoc" for this
 *     control — exactly the shape of a real doc comment in the kit.
 */
/* (d) SELF-REFERENTIAL: do not write the selector inline, because the scanner
 *     will harvest data-testid="phantom-from-warning-comment" out of THIS very
 *     comment. (The old scanner did precisely that. Twice.) */
const CONST = 'interpolated'
export function Fixture({ x }: { x?: string }) {
  // (c) a template interpolation inside a querySelector — never a rendered id
  document.querySelector(\`[data-testid="\${CONST}-row"]\`)
  return (
    <div>
      {/* NEGATIVE CONTROLS — these two are REAL and must survive */}
      <div data-testid="real-attribute-id" />
      <span data-testid={x ?? 'real-fallback-id'} />
    </div>
  )
}
`)
  const ids = [...collectTestIds([file])].sort()
  assert.deepEqual(
    ids,
    ['real-attribute-id', 'real-fallback-id'],
    'exactly the two real ids — no phantom from any comment or interpolation, ' +
      'and no real id dropped',
  )
  cleanup()
})

test('TEST-5b every phantom shape is individually absent', () => {
  const { file, cleanup } = fixture(`
// data-testid="p-line"
/** data-testid="p-jsdoc" */
/* data-testid="p-block" */
export const S = () => {
  const k = 'v'
  document.querySelector(\`[data-testid="\${k}-row-\${k}"]\`)
  return <i data-testid="kept" />
}
`)
  const ids = collectTestIds([file])
  for (const phantom of ['p-line', 'p-jsdoc', 'p-block'])
    assert.equal(ids.has(phantom), false, `${phantom} must not be harvested`)
  for (const id of ids)
    assert.equal(id.includes('${'), false, `no interpolation survived: ${id}`)
  assert.deepEqual([...ids], ['kept'])
  cleanup()
})

// ---------------------------------------------------------------------------
// TEST-22 — value-position semantics. Every negative below is a real
// over-collection an earlier draft of this pass actually produced.
// ---------------------------------------------------------------------------
test('TEST-22 ternary ARMS are collected; the ternary CONDITION is not', () => {
  const { file, cleanup } = fixture(`
export const A = ({ status }: { status: string }) => (
  <p data-testid={status === 'failed' ? 'boot-failed' : 'boot-starting'} />
)
`)
  assert.deepEqual([...collectTestIds([file])].sort(), ['boot-failed', 'boot-starting'])
  cleanup()
})

test('TEST-22b ?? and || operands are collected (the id the regex silently MISSED)', () => {
  const { file, cleanup } = fixture(`
export const A = ({ testid }: { testid?: string }) => (
  <><b data-testid={testid ?? 'default-nullish'} /><i data-testid={testid || 'default-or'} /></>
)
`)
  assert.deepEqual(
    [...collectTestIds([file])].sort(),
    ['default-nullish', 'default-or'],
  )
  cleanup()
})

test('TEST-22c a CALL ARGUMENT is not a value position', () => {
  const { file, cleanup } = fixture(`
export const A = ({ tid }: { tid: (s: string) => string }) => (
  <b data-testid={tid('toggle')} />
)
`)
  assert.deepEqual([...collectTestIds([file])], [], 'tid("toggle") yields no id')
  cleanup()
})

test('TEST-22d a TEMPLATE SPAN is not a value position', () => {
  const { file, cleanup } = fixture(`
export const A = (props: Record<string, string>) => (
  <b data-testid={\`\${props['data-testid']}-root\`} />
)
`)
  const ids = collectTestIds([file])
  assert.equal(ids.has('data-testid'), false, 'the span expression is not an id')
  assert.deepEqual([...ids], [])
  cleanup()
})

test('TEST-22e parenthesis / as / non-null wrappers are transparent', () => {
  const { file, cleanup } = fixture(`
export const A = () => (
  <><b data-testid={('wrapped-paren')} /><i data-testid={'wrapped-as' as string} /></>
)
`)
  assert.deepEqual(
    [...collectTestIds([file])].sort(),
    ['wrapped-as', 'wrapped-paren'],
  )
  cleanup()
})

test('collectTestIds extracts the plain attribute forms (= double + single quote)', () => {
  const { file, cleanup } = fixture(
    `export const A = () => (<div><b data-testid="alpha" /><i data-testid='gamma' /></div>)`,
  )
  assert.deepEqual([...collectTestIds([file])].sort(), ['alpha', 'gamma'])
  cleanup()
})

test('the QUOTED-KEY object form is now collected (it was a regex blind spot)', () => {
  // The predecessor test asserted this form was NOT captured, explicitly because
  // that "mirrors the original app generator's regex" — i.e. it encoded a scanner
  // artifact, not desired behaviour. `{'data-testid': 'beta'}` spread onto an
  // element renders a real attribute, so the AST pass collects it. Verified to
  // add ZERO ids on the real configured trees (see DECISIONS.md DEC-1).
  const { file, cleanup } = fixture(`export const o = { 'data-testid': 'beta' }`, 'ts')
  assert.deepEqual([...collectTestIds([file])], ['beta'])
  cleanup()
})

test('derived/template ids and non-literal expressions yield nothing', () => {
  const { file, cleanup } = fixture(`
export const A = ({ row, someVar }: { row: string; someVar: string }) => (
  <><b data-testid={\`\${row}-cell\`} /><i data-testid={someVar} /><u data-testid="kept" /></>
)
`)
  assert.deepEqual([...collectTestIds([file])], ['kept'])
  cleanup()
})

test('collectTestIdSites reports file:line for each id', () => {
  const { file, cleanup } = fixture(`\n\n<div data-testid="located" />\n`)
  const sites = collectTestIdSites([file])
  assert.equal(sites.get('located')[0].file, file)
  assert.equal(sites.get('located')[0].line, 3)
  cleanup()
})

// ---------------------------------------------------------------------------
// TEST-23 — id-shape validation at render time (defense-in-depth).
// ---------------------------------------------------------------------------
test('TEST-23 renderRegistry throws on a malformed id, naming it', () => {
  assert.throws(
    () => renderRegistry(['ok-id', '${testid}-row-${cssEscape(rk)}']),
    e =>
      /fail/.test(e.message) && e.message.includes('${testid}-row-${cssEscape(rk)}'),
    'the offending id must appear in the error',
  )
})

test('TEST-23b assertIdShapes names the source file:line when sites are known', () => {
  const sites = new Map([['bad id', [{ file: '/x/A.tsx', line: 42 }]]])
  assert.throws(
    () => assertIdShapes(['bad id'], sites),
    e => e.message.includes('/x/A.tsx:42'),
  )
})

test('TEST-23c every phantom the old scanner emitted violates ID_SHAPE', () => {
  // Proves the shape guard is a genuine second line of defence: it would have
  // caught all of them with the regex still in place.
  for (const phantom of [
    '${testid}-row-${cssEscape(rk)}',
    'chat-pane-${idx}',
    'kb-hit-source-${n - 1}',
  ])
    assert.equal(ID_SHAPE.test(phantom), false, phantom)
})

test('renderRegistry is deterministic + emits the KnownTestId union', () => {
  const body = renderRegistry(['a-btn', 'b-btn'])
  assert.match(body, /export const TEST_IDS = \[\n {2}"a-btn",\n {2}"b-btn",\n\] as const/)
  assert.match(body, /export type KnownTestId = \(typeof TEST_IDS\)\[number\]/)
  assert.match(body, /export const isKnownTestId/)
  assert.match(body, /2 static data-testid ids/)
})

test('collectSourceFiles skips gallery seeds, generated output, tests + src/dev', () => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  const src = path.join(root, 'src')
  fs.mkdirSync(path.join(src, 'dev', 'gallery'), { recursive: true })
  fs.mkdirSync(path.join(src, 'tests'), { recursive: true })
  fs.mkdirSync(path.join(src, 'modules'), { recursive: true })
  fs.writeFileSync(path.join(src, 'Keep.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'testIds.generated.ts'), 'x')
  fs.writeFileSync(path.join(src, 'modules', 'gallery.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'dev', 'gallery', 'Story.tsx'), 'x')
  fs.writeFileSync(path.join(src, 'tests', 'a.ts'), 'x')
  const got = collectSourceFiles(src).map(f => path.basename(f))
  assert.deepEqual(got, ['Keep.tsx'])
  fs.rmSync(root, { recursive: true, force: true })
})

test('collectSourceFiles skips CO-LOCATED test suites, wherever they sit', () => {
  // The class the `tests`-directory skip above does NOT cover, and the one that was
  // actually live: a suite beside the component it exercises. `@ziee/kit` co-locates
  // its component tests under `src/kit/`, ziee scans that tree via `kitTestIds`, and
  // `sheet-bottom-track.test.tsx`'s `data-testid="body-child"` fixture was harvested
  // straight into the production `KnownTestId` union.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-colocated-'))
  const src = path.join(root, 'src')
  fs.mkdirSync(path.join(src, 'kit'), { recursive: true })

  // Every co-located suite spelling, at depth, in both extensions.
  for (const f of [
    'sheet-bottom-track.test.tsx',
    'table-view-core.test.ts',
    'portal.spec.tsx',
    'view.spec.ts',
  ])
    fs.writeFileSync(path.join(src, 'kit', f), 'x')

  // NEGATIVE CONTROL — real source whose NAME merely contains "test"/"spec" must
  // still be collected. A substring match here would silently delete real ids from
  // the registry, which is the opposite failure and just as bad.
  for (const f of ['TestModeBanner.tsx', 'latest.ts', 'Inspector.tsx', 'spectrum.ts'])
    fs.writeFileSync(path.join(src, 'kit', f), 'x')

  const got = collectSourceFiles(src)
    .map(f => path.basename(f))
    .sort()
  assert.deepEqual(got, [
    'Inspector.tsx',
    'TestModeBanner.tsx',
    'latest.ts',
    'spectrum.ts',
  ])
  fs.rmSync(root, { recursive: true, force: true })
})

// ---------------------------------------------------------------------------
// TEST-21 — GOLDEN set-equality against the REAL configured trees, with every
// removed and added id asserted BY NAME.
// ---------------------------------------------------------------------------

/** The 3 phantoms the text scan harvested that are NOT real ids. */
const REMOVED_PHANTOMS = [
  '${testid}-row-${cssEscape(rk)}', // kit/src/kit/table.tsx — querySelector template
  'chat-pane-${idx}', // chat/extensions/keyboard/extension.tsx — same shape
  'kb-hit-source-${n - 1}', // chat/core/utils/CitationChip.tsx — same shape
]
/** The 6 REAL ids the text scan silently MISSED (`??` / ternary value positions). */
const ADDED_REAL = [
  'chat-single-drop-column',
  'desktop-bootstrap-failed',
  'desktop-bootstrap-starting',
  'memory-core-block-create-dialog',
  'memory-core-block-edit-dialog',
  'settings-page-title',
]

test('TEST-21 [golden] the collector reproduces the committed registry exactly', () => {
  // Resolve the app's config the way the generator's main() does, from the ui
  // workspace (the committed registry is the sorted UNION, identical from either
  // workspace cwd — see the generator header).
  const uiCwd = path.resolve(HERE, '../../../../src-app/ui')
  if (!fs.existsSync(path.join(uiCwd, 'gallery.config.json'))) {
    // Package consumed standalone (no app tree) — the fixture tests above still
    // fully cover the collector; skip only the app-tree golden.
    return
  }
  const CFG = resolveGalleryConfig(uiCwd)
  const R = p => path.resolve(uiCwd, p)
  const trees = [CFG.srcDir, ...(CFG.extraTrees ?? []), ...(CFG.kitTestIds ?? [])].map(R)
  const files = trees.flatMap(r => collectSourceFiles(r))
  const got = [...collectTestIds(files)].sort()

  const out = R(CFG.testidOut)
  const committed = fs.readFileSync(out, 'utf-8')
  const inRegistry = [...committed.matchAll(/^ {2}"(.+)",$/gm)].map(m =>
    JSON.parse(`"${m[1]}"`),
  )
  assert.deepEqual(
    got,
    inRegistry.slice().sort(),
    'collector output must equal the committed registry (run `npm run gen:testid-registry`)',
  )

  // Every removed phantom is GONE, by name.
  for (const p of REMOVED_PHANTOMS)
    assert.equal(got.includes(p), false, `phantom must be absent: ${p}`)
  // Every recovered real id is PRESENT, by name — this half is what fails a
  // "fix" that removed phantoms by dropping real ids.
  for (const a of ADDED_REAL)
    assert.equal(got.includes(a), true, `real id must be present: ${a}`)
  // And the whole set satisfies the shape guard.
  assert.doesNotThrow(() => assertIdShapes(got))
})

// ---------------------------------------------------------------------------
// TEST-25 — ts-morph resolves from @ziee/gallery's own declared dependencies.
// ---------------------------------------------------------------------------
test('TEST-25 ts-morph is a DECLARED dependency of @ziee/gallery', () => {
  const pkg = JSON.parse(
    fs.readFileSync(path.resolve(HERE, '../package.json'), 'utf-8'),
  )
  const declared = {
    ...(pkg.dependencies ?? {}),
    ...(pkg.devDependencies ?? {}),
    ...(pkg.peerDependencies ?? {}),
  }
  assert.ok(
    declared['ts-morph'],
    'ts-morph must be declared — the AST pass currently resolves only by a ' +
      'root-workspace hoist accident, which breaks the package standalone',
  )
})
