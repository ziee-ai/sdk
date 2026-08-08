/**
 * Generate testIds.generated.ts — a typed registry of every STATIC data-testid
 * literal across the app trees (ui + desktop) AND the kit/shell PACKAGE trees,
 * for agentic + i18n-safe testing.
 *
 * Why: tests must select by data-testid (visible text/labels break under i18n).
 * This emits a union of all known ids + a Playwright helper, so a test that
 * targets a non-existent or misspelled testid fails to COMPILE instead of flakily
 * at runtime. Derived/template ids (`${container}-row-${k}`) aren't enumerable
 * statically, so the helper also accepts an arbitrary string (autocompleting the
 * known ones).
 *
 * Config-driven (moved into `@ziee/gallery/scripts` from the app). The walked
 * roots + the output path all come from `resolveGalleryConfig(cwd)`:
 *   - app trees   = `srcDir` + `extraTrees`   (ui/src, desktop/ui/src);
 *   - package trees = `kitTestIds`             (kit/src, shell/src — the kit
 *                     components moved into packages, so their own ids are the
 *                     source of truth for kit-component testids);
 *   - output      = `testidOut`                (the kit package's committed
 *                     `testIds.generated.ts`, imported app-side as
 *                     `@ziee/kit/testIds.generated`).
 * Every default reproduces the pre-package in-app behavior (walk `srcDir`, write
 * `<srcDir>/components/ui/testIds.generated.ts`), so a config-less app is
 * unchanged. Run from BOTH the ui and desktop workspace cwds (each supplies the
 * mirrored relative roots) — the sorted UNION is identical, so the single
 * committed registry is byte-stable regardless of which workspace's `--check` ran.
 *
 * Run: node gen-testid-registry.mjs        (write)
 *      node gen-testid-registry.mjs --check (drift guard)
 */
import { pathToFileURL } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import { Project, SyntaxKind, Node } from 'ts-morph'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

/**
 * The shape every real testid has. Enforced at RENDER time (not only at collect
 * time) as defense-in-depth: even if a future collector regresses, an id that
 * cannot be a real DOM attribute value can never reach the committed registry.
 * Every one of the phantoms this generator used to harvest violated this.
 */
export const ID_SHAPE = /^[a-zA-Z0-9_-]+$/

/** A cheap pre-filter: a file with no `data-testid` text anywhere cannot contain
 *  a `data-testid` attribute, so it never needs to be parsed. Text-matching is
 *  sound HERE (it can only skip files, never accept a phantom) and keeps the AST
 *  pass fast over ~2.4k source files. */
const MENTIONS_TESTID = 'data-testid'

/**
 * Recursively collect `.ts/.tsx/.jsx` source files under `dir`, skipping build/vcs
 * dirs, the `tests` tree, the dev-only gallery (`src/dev`), per-module `gallery.*`
 * seed files, and any `testIds.generated.ts` (so walking a package's own output
 * never feeds it back). Pure — exported for the unit test.
 */
export function collectSourceFiles(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      // Skip the dev-only component gallery (its testids are gallery-internal and
      // must not expand the app's typed production registry). Path-anchored to a
      // `dev` dir directly under a `src` root so an unrelated `dev` dir isn't
      // silently dropped.
      const isGalleryDev = e === 'dev' && /[\\/]src$/.test(dir)
      if (
        !['node_modules', 'dist', 'build', '.git', 'tests'].includes(e) &&
        !isGalleryDev
      )
        collectSourceFiles(full, acc)
    } else if (
      /\.(tsx|jsx|ts)$/.test(e) &&
      // Per-module `gallery.tsx` seeds carry gallery-internal testids (a seeded
      // dialog's `testid:`); they must not expand the production registry.
      e !== 'gallery.tsx' &&
      e !== 'gallery.ts' &&
      e !== 'testIds.generated.ts'
    )
      acc.push(full)
  }
  return acc
}

/**
 * True for the two literal node kinds that denote a fixed string. A
 * `TemplateExpression` (one WITH `${…}` spans) is deliberately excluded — an
 * interpolated id is not statically enumerable, and treating its raw text as an
 * id is exactly the phantom this generator used to emit.
 */
const isLiteral = n =>
  Node.isStringLiteral(n) || Node.isNoSubstitutionTemplateLiteral(n)

/**
 * Walk the VALUE POSITIONS of a `data-testid` initializer — the sub-expressions
 * whose result can actually BECOME the attribute's value — and yield each fixed
 * string among them.
 *
 * Value positions are: the literal itself; both arms of a ternary; both operands
 * of `??` / `||`; and the inner expression of a parenthesis / `as` / `!` / satisfies
 * wrapper. Everything else is NOT a value position and yields nothing:
 *
 *   - a ternary CONDITION   — `s === 'failed' ? 'a' : 'b'` must not yield `failed`
 *   - a call ARGUMENT       — `tid('toggle')` must not yield `toggle`
 *   - a template SPAN       — `` `${p['data-testid']}-root` `` must not yield `data-testid`
 *   - a COMMENT             — structurally unreachable: trivia is not a node
 *
 * Those four are not hypothetical. The first three were produced by an earlier
 * draft of this pass that walked every descendant literal; the fourth is the
 * defect this rewrite exists to fix. A hand-rolled comment-stripper would be a
 * text scan with its own evasion space — here comments cost nothing to exclude
 * because they were never in the tree to begin with.
 */
function eachValueLiteral(node, visit) {
  if (!node) return
  if (isLiteral(node)) return visit(node)
  if (
    Node.isParenthesizedExpression(node) ||
    Node.isAsExpression(node) ||
    Node.isNonNullExpression(node) ||
    Node.isSatisfiesExpression(node) ||
    Node.isTypeAssertion(node)
  )
    return eachValueLiteral(node.getExpression(), visit)
  if (Node.isConditionalExpression(node)) {
    eachValueLiteral(node.getWhenTrue(), visit)
    eachValueLiteral(node.getWhenFalse(), visit)
    return
  }
  if (Node.isBinaryExpression(node)) {
    const op = node.getOperatorToken().getKind()
    if (
      op === SyntaxKind.QuestionQuestionToken ||
      op === SyntaxKind.BarBarToken
    ) {
      eachValueLiteral(node.getLeft(), visit)
      eachValueLiteral(node.getRight(), visit)
    } else if (op === SyntaxKind.AmpersandAmpersandToken) {
      // `cond && 'id'` — only the RIGHT operand can become the value; the left
      // is a condition (the same reason a ternary's condition is excluded).
      // Omitting `&&` entirely silently DROPPED such ids from the shared
      // registry, and a dropped id is invisible: the shape guard can only catch
      // a malformed id, never a missing one.
      eachValueLiteral(node.getRight(), visit)
    }
  }
}

/** The property/attribute name that carries a testid, in any spelling. */
const isTestIdName = nameNode => {
  if (!nameNode) return false
  const raw = Node.isStringLiteral(nameNode)
    ? nameNode.getLiteralValue()
    : nameNode.getText()
  return raw === 'data-testid'
}

/** Visit every `data-testid` value literal in one parsed source file. */
function eachTestIdInFile(sf, visit) {
  for (const attr of sf.getDescendantsOfKind(SyntaxKind.JsxAttribute)) {
    if (!isTestIdName(attr.getNameNode())) continue
    const init = attr.getInitializer()
    if (!init) continue
    if (Node.isJsxExpression(init)) eachValueLiteral(init.getExpression(), visit)
    else eachValueLiteral(init, visit)
  }
  for (const pa of sf.getDescendantsOfKind(SyntaxKind.PropertyAssignment)) {
    if (!isTestIdName(pa.getNameNode())) continue
    eachValueLiteral(pa.getInitializer(), visit)
  }
}

/**
 * Collect every static `data-testid` value across `files`, as a Map of
 * id → [{ file, line }] so a bad id can be reported at its source.
 *
 * This is an AST pass (ts-morph — already a dependency, already how the sibling
 * `gen-state-matrix.mjs` reads these trees), NOT a text scan. The predecessor
 * regex matched `data-testid` followed by a quoted value ANYWHERE in the file
 * text, so it harvested ids out of comments and out of `querySelector` template
 * strings, and it simultaneously MISSED every id written in a `??`/ternary value
 * position because those do not put a quote immediately after the attribute.
 */
export function collectTestIdSites(files) {
  const sites = new Map()
  const project = new Project({
    skipAddingFilesFromTsConfig: true,
    // `jsx: preserve` so `.tsx` parses as JSX; `allowJs` for the `.jsx` trees.
    compilerOptions: { allowJs: true, jsx: 1 },
  })
  for (const f of files) {
    let text
    try {
      text = fs.readFileSync(f, 'utf-8')
    } catch {
      continue
    }
    if (!text.includes(MENTIONS_TESTID)) continue
    const sf = project.createSourceFile(f, text, { overwrite: true })
    eachTestIdInFile(sf, lit => {
      const id = lit.getLiteralValue()
      const line = sf.getLineAndColumnAtPos(lit.getStart()).line
      if (!sites.has(id)) sites.set(id, [])
      sites.get(id).push({ file: f, line })
    })
    // Drop the parsed file — 2.4k source files held at once is needless memory.
    project.removeSourceFile(sf)
  }
  return sites
}

/** Extract every static `data-testid` literal from a set of files into a Set. */
export function collectTestIds(files) {
  return new Set(collectTestIdSites(files).keys())
}

/**
 * Defense-in-depth (independent of the collector): refuse any id that cannot be
 * a real DOM attribute value. Every phantom the old text scan produced violated
 * this — `${testid}-row-${cssEscape(rk)}`, `chat-pane-${idx}`, … — so this check
 * would have caught all of them even with the regex still in place. It is
 * expected to be permanently silent; it exists so a future collector regression
 * fails loudly instead of shipping into the shared registry.
 */
export function assertIdShapes(ids, sites) {
  const bad = [...ids].filter(id => !ID_SHAPE.test(id))
  if (!bad.length) return
  const where = id => {
    const s = sites?.get(id)
    return s?.length ? ` (${s[0].file}:${s[0].line})` : ''
  }
  throw new Error(
    `gen-testid-registry: ${bad.length} id(s) fail ${ID_SHAPE} — a testid must be a plain ` +
      `attribute value. This means the collector harvested something that is not an id ` +
      `(an interpolation, a comment, a selector fragment):\n` +
      bad.map(id => `  ${JSON.stringify(id)}${where(id)}`).join('\n'),
  )
}

/**
 * Render the registry file body from a sorted id list (byte-stable format).
 * Throws (naming the id + its source) if any id fails `ID_SHAPE` — the shape
 * guard lives HERE, at the last point before the shared registry is written, so
 * no caller can bypass it.
 */
export function renderRegistry(sorted, sites) {
  assertIdShapes(sorted, sites)
  return `// AUTO-GENERATED by scripts/gen-testid-registry.mjs — DO NOT EDIT.
// Run \`npm run gen:testid-registry\` to refresh. ${sorted.length} static data-testid ids
// across the ui + desktop trees. Tests select by these (i18n-safe), with compile-time
// typo-checking via the KnownTestId union.

export const TEST_IDS = [
${sorted.map((id) => `  ${JSON.stringify(id)},`).join('\n')}
] as const

/** Every static data-testid literal in the app. Derived (\`\${id}-row-\${k}\`) ids are NOT listed. */
export type KnownTestId = (typeof TEST_IDS)[number]

/** Accepts a known id (autocompleted) OR any string (for derived/template ids). */
export type TestIdLike = KnownTestId | (string & {})

const KNOWN = new Set<string>(TEST_IDS)
/** Runtime guard — true when \`id\` is a statically-known testid. */
export const isKnownTestId = (id: string): id is KnownTestId => KNOWN.has(id)
`
}

const isMain = import.meta.url === pathToFileURL(process.argv[1]).href
if (isMain) {
  const CFG = resolveGalleryConfig()
  const resolve = (p) => path.resolve(CFG.__cwd, p)

  // App trees (srcDir + extraTrees) UNION the package trees (kitTestIds).
  const appTrees = [CFG.srcDir, ...(CFG.extraTrees ?? [])].map(resolve)
  const pkgTrees = (CFG.kitTestIds ?? []).map(resolve)
  const out = resolve(
    CFG.testidOut ?? path.join(CFG.srcDir, 'components/ui/testIds.generated.ts'),
  )

  const files = [...appTrees, ...pkgTrees].flatMap((r) => collectSourceFiles(r))
  const sites = collectTestIdSites(files)
  const sorted = [...sites.keys()].sort()
  const body = renderRegistry(sorted, sites)

  const check = process.argv.includes('--check')
  if (check) {
    const cur = fs.existsSync(out) ? fs.readFileSync(out, 'utf-8') : ''
    if (cur.trim() !== body.trim()) {
      console.error(
        'testIds.generated.ts is stale — run `npm run gen:testid-registry` and commit.',
      )
      process.exit(1)
    }
    console.log(`testIds.generated.ts up to date (${sorted.length} ids).`)
  } else {
    fs.mkdirSync(path.dirname(out), { recursive: true })
    fs.writeFileSync(out, body)
    console.log(`Wrote ${out} (${sorted.length} ids).`)
  }
}
