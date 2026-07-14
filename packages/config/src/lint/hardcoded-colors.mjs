/**
 * Guardrail: NO hardcoded colors in app code.
 *
 * Colors must come from the design tokens (semantic Tailwind classes like `bg-primary`,
 * `text-muted-foreground`, `border-border`) or the kit — never a raw Tailwind palette hue
 * (`bg-blue-500`, `text-red-600`), an arbitrary color value (`bg-[#1e90ff]`), or an inline
 * `style` color property. Hardcoded colors bypass the accent/theme system (they don't respond
 * to the user's accent or to dark mode) and break visual consistency.
 *
 * Applies to ALL JSX elements (div/span/kit components alike) in app code. Opt out per element
 * with the `data-allow-custom-color` flag (for genuinely-dynamic colors, e.g. a swatch picker).
 * kit/ + shadcn/ are excluded — they DEFINE the tones the rest of the app consumes.
 *
 * Biome's GritQL plugins can't introspect className strings / style objects in this build, so
 * this is a TS-compiler-API AST check, wired into `check` (and CI).
 *
 *   node scripts/lint-hardcoded-colors.mjs            # fail on any violation
 */
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'

import { parseRoots } from './roots.mjs'

const require = createRequire(import.meta.url)
const ts = require('typescript')

// Parameterized over the app's src tree(s) via `--root=<dir>` (repeatable).
const ROOTS = parseRoots()
const OPT_OUT = 'data-allow-custom-color'

// Tailwind default palette hues ONLY — NOT the semantic tokens (primary/secondary/muted/
// accent/destructive/background/foreground/card/popover/border/input/ring/sidebar*) and NOT
// any custom `@theme { --color-X }` token names (those are intentional, theme-aware).
const HUES =
  'slate|gray|zinc|neutral|stone|red|orange|amber|yellow|lime|green|emerald|teal|cyan|sky|blue|indigo|violet|purple|fuchsia|pink|rose'
// Color-affecting Tailwind utility prefixes (per the Tailwind color-utilities reference):
// bg / text / border(+per-side & per-axis: t r b l x y s e) / ring / ring-offset / divide /
// outline / gradient stops (from via to) / fill / stroke / decoration / placeholder / caret /
// accent (form) / shadow (colored). HUE-anchored so size/style utilities (ring-2, shadow-lg,
// decoration-dotted, from-10%) are NOT flagged.
const PREFIX = [
  'bg', 'text', 'border(?:-(?:t|r|b|l|x|y|s|e))?', 'ring(?:-offset)?', 'divide', 'outline',
  'from', 'via', 'to', 'fill', 'stroke', 'decoration', 'placeholder', 'caret', 'accent', 'shadow',
].join('|')
const PALETTE = new RegExp(`^(?:${PREFIX})-(?:${HUES})(?:-\\d{1,3})?(?:\\/\\d{1,3})?$`)
// NOTE: white/black/transparent/current/inherit are intentionally NOT banned — they're neutral
// (not brand hues), commonly legitimate (an icon on a colored surface), and banning them buries
// the real signal (hardcoded hues that bypass the accent/theme) under opt-out noise.
// arbitrary color values: bg-[#fff], text-[rgb(...)], border-[hsl(...)], …  (NOT bg-(--var),
// which references a token).
const ARBITRARY = new RegExp(
  `^(?:${PREFIX})-\\[(?:#|rgb|rgba|hsl|hsla|oklch|oklab|lab|lch|hwb|color)`,
  'i',
)
// style object keys that set a color (full CSS <color> property set incl. logical + webkit + SVG).
const STYLE_COLOR_KEYS = new Set([
  'color', 'background', 'backgroundColor', 'accentColor', 'caretColor', 'columnRuleColor',
  'outlineColor', 'textDecorationColor', 'boxShadow', 'textShadow', 'fill', 'stroke',
  'floodColor', 'lightingColor', 'stopColor', 'scrollbarColor',
  'webkitTextFillColor', 'webkitTextStrokeColor', 'WebkitTextFillColor', 'WebkitTextStrokeColor',
  'borderColor', 'borderTopColor', 'borderRightColor', 'borderBottomColor', 'borderLeftColor',
  'borderBlockColor', 'borderBlockStartColor', 'borderBlockEndColor',
  'borderInlineColor', 'borderInlineStartColor', 'borderInlineEndColor',
])

const isBannedClass = (tok) => {
  // strip Tailwind variant prefixes (hover: / dark: / md: / group-hover: …) and leading `!`.
  const core = tok.includes(':') ? tok.slice(tok.lastIndexOf(':') + 1) : tok
  const bare = core.startsWith('!') ? core.slice(1) : core
  return PALETTE.test(bare) || ARBITRARY.test(bare)
}

function findFiles(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (
        !['node_modules', 'dist', 'build', '.git', 'tests', 'shadcn'].includes(e) &&
        !full.endsWith(path.join('components', 'ui', 'kit'))
      ) {
        findFiles(full, acc)
      }
    } else if (/\.(tsx|jsx)$/.test(e) && !e.endsWith('.generated.ts')) {
      acc.push(full)
    }
  }
  return acc
}

const violations = []

function collectStringLiterals(node, out) {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) out.push(node.text)
  else if (ts.isTemplateExpression(node)) {
    out.push(node.head.text)
    for (const span of node.templateSpans) out.push(span.literal.text)
  }
  ts.forEachChild(node, (c) => collectStringLiterals(c, out))
}

function attrName(attr) {
  return attr.name && ts.isIdentifier(attr.name)
    ? attr.name.text
    : attr.name && attr.name.namespace
      ? `${attr.name.namespace.text}:${attr.name.name.text}`
      : attr.name?.getText?.() ?? ''
}

function checkOpeningLike(attrsNode, sf) {
  if (!attrsNode) return
  const attrs = attrsNode.properties.filter(ts.isJsxAttribute)
  const optedOut = attrs.some((a) => attrName(a) === OPT_OUT)
  if (optedOut) return

  for (const a of attrs) {
    const name = attrName(a)
    if (name === 'className' && a.initializer) {
      const strings = []
      collectStringLiterals(a.initializer, strings)
      const hits = new Set()
      for (const s of strings) for (const tok of s.split(/\s+/)) if (tok && isBannedClass(tok)) hits.add(tok)
      if (hits.size) report(a, sf, `hardcoded color class(es): ${[...hits].join(' ')}`)
    } else if (name === 'style' && a.initializer && ts.isJsxExpression(a.initializer)) {
      const expr = a.initializer.expression
      if (expr && ts.isObjectLiteralExpression(expr)) {
        const keys = expr.properties
          .filter((p) => p.name && (ts.isIdentifier(p.name) || ts.isStringLiteral(p.name)))
          .map((p) => p.name.text)
          .filter((k) => STYLE_COLOR_KEYS.has(k))
        if (keys.length) report(a, sf, `inline style color prop(s): ${keys.join(', ')}`)
      }
    }
  }
}

function report(node, sf, msg) {
  const { line, character } = sf.getLineAndCharacterOfPosition(node.getStart(sf))
  violations.push({ file: sf.fileName, line: line + 1, col: character + 1, msg })
}

for (const root of ROOTS) {
  for (const file of findFiles(root)) {
    const sf = ts.createSourceFile(file, fs.readFileSync(file, 'utf-8'), ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
    const visit = (node) => {
      if (ts.isJsxElement(node)) checkOpeningLike(node.openingElement.attributes, sf)
      else if (ts.isJsxSelfClosingElement(node)) checkOpeningLike(node.attributes, sf)
      ts.forEachChild(node, visit)
    }
    visit(sf)
  }
}

if (violations.length) {
  console.error(`\n[no-hardcoded-colors] ✗ ${violations.length} violation(s):\n`)
  for (const v of violations) {
    console.error(`  ${path.relative(process.cwd(), v.file)}:${v.line}:${v.col}  ${v.msg}`)
  }
  console.error(
    `\nUse semantic token classes (bg-primary, text-muted-foreground, border-border) or the kit.\n` +
      `For a genuinely-dynamic color, add the \`${OPT_OUT}\` flag to that element.\n`,
  )
  process.exit(1)
}
console.log('[no-hardcoded-colors] ✓ no hardcoded colors in app code.')
