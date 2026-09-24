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
  isTestSourceFile,
  renderRegistry,
  resolveRegistryScope,
} from './gen-testid-registry.mjs'

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
// TEST-21 — GOLDEN set-equality against the REAL committed registry, driven by
// the SAME `resolveRegistryScope()` the generator's main() uses, so the golden
// can never drift from the generator's root logic.
//
// The committed registry in the kit's own tree is the SHARED surface (every
// consumer imports `@ziee/kit/testIds.generated`), so its golden is built here
// from SDK-INTERNAL paths and ALWAYS RUNS — in the sdk repo no less than inside
// a consumer. A second block covers a consumer app where one actually exists
// (dental mounts the sdk as a submodule, so `../../../../src-app/ui` resolves).
//
// The golden deliberately names NO consumer-app ids (the old TEST-21 pinning
// REMOVED_PHANTOMS/ADDED_REAL asserted ZIEE's id set — it could never pass for
// any other consumer). The mechanics those by-name assertions covered are
// fixture-covered right here: TEST-5/TEST-5b (comments + interpolations yield
// nothing), the TEST-22 series (value positions: ternary arms, `??`/`||`, never
// conditions/args/template spans) and TEST-23c (the three exact phantoms the
// old scanner harvested — `${testid}-row-${cssEscape(rk)}`, `chat-pane-${idx}`,
// `kb-hit-source-${n - 1}` — each FAILS `ID_SHAPE`). The golden itself still
// guards the regressions that matter set-wise: exact equality with the committed
// file (nothing added, nothing dropped) and the shape guard over the whole set.
// ---------------------------------------------------------------------------

/** Parse the id list out of a committed registry body (byte-stable format). */
function parseCommittedRegistry(body) {
  const ids = [...body.matchAll(/^ {2}"(.+)",$/gm)].map(m => JSON.parse(`"${m[1]}"`))
  return ids.sort()
}

test('TEST-21 [golden] sdk-local scope reproduces the committed kit registry exactly (always runs)', () => {
  // The scope is built from SDK-INTERNAL paths anchored at this test file, so it
  // needs no consumer app layout and never returns early: the output IS the kit's
  // own committed shared surface, `resolveRegistryScope` must see it as such, and
  // the walk must therefore be the kit/shell package trees ONLY.
  const scope = resolveRegistryScope({
    __cwd: HERE,
    srcDir: HERE,
    kitTestIds: [
      path.resolve(HERE, '../../kit/src'),
      path.resolve(HERE, '../../shell/src'),
    ],
    testidOut: path.resolve(HERE, '../../kit/src/testIds.generated.ts'),
  })
  assert.equal(
    scope.isKitSurface,
    true,
    'an output inside a kitTestIds root is the kit shared surface',
  )
  const got = [...collectTestIds(scope.files)].sort()
  const committed = fs.readFileSync(scope.out, 'utf-8')
  assert.deepEqual(
    got,
    parseCommittedRegistry(committed),
    'collector output must equal the committed kit registry (consumer app: run `npm run gen:testid-registry`; sdk repo: run `node packages/gallery/scripts/gen-testid-registry.mjs` from a dir with a kit-pointing config)',
  )
  // Two by-name checks against the REAL source, both directions: an id that is a
  // `??`-arm in shell source must be PRESENT; the exact phantom the old text scan
  // harvested out of kit's table.tsx template must be ABSENT. (Written as a plain
  // string — this file lives under scripts/, never walked by the collector.)
  assert.equal(got.includes('settings-page-title'), true, 'real shell id must be present')
  assert.equal(
    got.includes('${testid}-row-${cssEscape(rk)}'),
    false,
    'the kit table.tsx phantom must be absent',
  )
  // The whole set satisfies the shape guard (defense-in-depth, unchanged).
  assert.doesNotThrow(() => assertIdShapes(got))
})

test('TEST-21b [golden consumer] the consumer app scope reproduces its committed registry exactly', (t) => {
  // Runs only where a consumer app layout exists (dental). In the sdk repo this
  // golden is absent by construction, which is fine: the sdk-local golden above is
  // the one that always runs, and no app-specific id is hardcoded here either —
  // the scope comes from the app's OWN gallery.config.json.
  const uiCwd = path.resolve(HERE, '../../../../src-app/ui')
  if (!fs.existsSync(path.join(uiCwd, 'gallery.config.json'))) {
    // No consumer app checked out here (package consumed standalone). The sdk-local
    // golden above already asserted the shared surface; this block asserts the
    // per-app union, which has no meaning without the app — report the skip
    // honestly instead of a vacuous pass.
    t.skip('no consumer app layout — consumer golden not applicable here')
    return
  }
  const scope = resolveRegistryScope(uiCwd)
  const got = [...collectTestIds(scope.files)].sort()
  const committed = fs.readFileSync(scope.out, 'utf-8')
  assert.deepEqual(
    got,
    parseCommittedRegistry(committed),
    "collector output must equal the consumer app's committed registry " +
      '(consumer app: run `npm run gen:testid-registry`; sdk repo: run ' +
      '`node packages/gallery/scripts/gen-testid-registry.mjs` from a dir with a kit-pointing config)',
  )
  assert.doesNotThrow(() => assertIdShapes(got))
})

// ---------------------------------------------------------------------------
// TEST-21c/TEST-21d — isKitSurface CONTAINMENT edges. Contained-ness is decided
// by `path.relative` after realpath canonicalization: a child NAMED `..weird` is
// INSIDE (its relative path merely STARTS with two dots), while a real `../sibling`
// escape is OUTSIDE. Both go through the PUBLIC `resolveRegistryScope` with a
// config-shaped object, exactly like the sdk-local golden does.
// ---------------------------------------------------------------------------
test('TEST-21c [containment] a child dir NAMED `..weird` with the out inside it is the kit surface', () => {
  const R = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-contain-'))
  try {
    const weird = path.join(R, '..weird')
    fs.mkdirSync(weird)
    const out = path.join(weird, 'testIds.generated.ts')
    const scope = resolveRegistryScope({
      __cwd: R,
      srcDir: R,
      kitTestIds: [R],
      testidOut: out,
    })
    assert.equal(
      scope.isKitSurface,
      true,
      'a child named `..weird` is INSIDE the root — only a real `../`/`..` escape is outside',
    )
  } finally {
    fs.rmSync(R, { recursive: true, force: true })
  }
})

test('TEST-21d [containment] an out at `../sibling` is NOT the kit surface', () => {
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-contain-'))
  try {
    const R = path.join(base, 'root')
    fs.mkdirSync(R)
    fs.mkdirSync(path.join(base, 'sibling'))
    const out = path.join(R, '..', 'sibling', 'testIds.generated.ts')
    const scope = resolveRegistryScope({
      __cwd: R,
      srcDir: R,
      kitTestIds: [R],
      testidOut: out,
    })
    assert.equal(
      scope.isKitSurface,
      false,
      'a real `../sibling` escape is OUTSIDE the root — never the kit surface',
    )
  } finally {
    fs.rmSync(base, { recursive: true, force: true })
  }
})

test('TEST-21e [containment] a symlinked kit root with a NOT-YET-CREATED output is the kit surface', (t) => {
  // The realpath-only canonicalization used to resolve the (always-existing) kit
  // root through the symlink but left the (not-yet-created, first-run) output on
  // its UNRESOLVED path — so a first-run write under a symlinked root read
  // `isKitSurface=false` and silently wrote the app∪kit UNION into the kit tree.
  // canonical() instead resolves the deepest EXISTING ancestor and appends the
  // unresolved tail, so root and output always agree.
  const base = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-symlink-'))
  try {
    const real = path.join(base, 'real')
    fs.mkdirSync(real)
    const link = path.join(base, 'link')
    try {
      fs.symlinkSync(real, link, 'dir')
    } catch {
      t.skip('symlink creation not permitted in this environment')
      return
    }
    const scope = resolveRegistryScope({
      __cwd: base,
      srcDir: base,
      kitTestIds: [path.join(base, 'link')],
      testidOut: path.join(base, 'link', 'testIds.generated.ts'),
    })
    assert.equal(
      scope.isKitSurface,
      true,
      'a symlinked kit root with a NOT-YET-CREATED output is still the kit surface',
    )
    // With the SAME symlinked root, an existing sibling through `../` stays
    // OUTSIDE — the symmetric edge, so the ancestor walk cannot over-correct.
    fs.mkdirSync(path.join(base, 'sibling'))
    const sibScope = resolveRegistryScope({
      __cwd: base,
      srcDir: base,
      kitTestIds: [path.join(base, 'link')],
      testidOut: path.join(base, 'link', '..', 'sibling', 'testIds.generated.ts'),
    })
    assert.equal(
      sibScope.isKitSurface,
      false,
      'a real `../sibling` escape out of a symlinked root is OUTSIDE — never the kit surface',
    )
  } finally {
    fs.rmSync(base, { recursive: true, force: true })
  }
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

test('collectSourceFiles skips CO-LOCATED test/spec/story files', () => {
  // The `tests` DIRECTORY skip only covers apps that keep tests in one tree. An
  // app whose vitest suites sit NEXT TO the component (testing-library house
  // style) had every throwaway id its tests invent — `data-testid="a"` — land in
  // the app's typed PRODUCTION registry. Observed in COMIZY: 171 ids generated,
  // 131 real, 40 contributed by co-located suites.
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'testid-'))
  const src = path.join(root, 'src', 'components')
  fs.mkdirSync(src, { recursive: true })
  fs.writeFileSync(path.join(src, 'Avatar.tsx'), '<div data-testid="avatar" />')
  fs.writeFileSync(path.join(src, 'Avatar.test.tsx'), '<Avatar data-testid="a" />')
  fs.writeFileSync(path.join(src, 'avatar.spec.ts'), 'data-testid="b"')
  fs.writeFileSync(path.join(src, 'Avatar.stories.tsx'), 'data-testid="story-only"')
  const files = collectSourceFiles(src)
  assert.deepEqual(files.map(f => path.basename(f)), ['Avatar.tsx'])
  assert.deepEqual([...collectTestIds(files)], ['avatar'])
  fs.rmSync(root, { recursive: true, force: true })
})

test('isTestSourceFile matches only the co-located suite suffixes', () => {
  for (const n of ['A.test.tsx', 'a.test.ts', 'a.spec.ts', 'a.spec.jsx', 'A.stories.tsx'])
    assert.equal(isTestSourceFile(n), true, n)
  // A production file whose NAME merely contains the word is kept.
  for (const n of ['Avatar.tsx', 'testUtils.ts', 'contest.tsx', 'spectrum.tsx', 'stories.ts'])
    assert.equal(isTestSourceFile(n), false, n)
})
