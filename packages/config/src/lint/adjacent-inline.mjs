/**
 * Guardrail (Layer 3 of the UI-defect detection system): adjacent inline pills
 * (Badge / Button / Tag / Chip / AddButton) that are DIRECT SIBLINGS inside a flex
 * ROW container carrying NO `gap-*` / `space-x-*` / `divide-x-*` utility — the
 * source-level cause of the zero-gap-adjacency defect (taxonomy A1, user miss #1:
 * the hardware "Disconnected"/"Connect" pair). The runtime geometry audit catches
 * the rendered symptom; this lint catches the markup before it ships.
 *
 * Rule: a JSX element whose className marks it a flex row (`flex` without
 * `flex-col`) and that has ≥2 consecutive pill children, but whose className has no
 * horizontal spacing utility, is flagged. A deliberately-joined group (segmented
 * control, button group) opts out with `data-allow-adjacent` on the container.
 *
 * kit/ + shadcn/ are excluded (they DEFINE the primitives). TS-compiler-API AST
 * (Biome GritQL can't introspect JSX ancestry in this build), wired into `check`.
 *
 *   node scripts/lint-adjacent-inline.mjs
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
const OPT_OUT = 'data-allow-adjacent'

// Inline "pill" components whose adjacency without a gap makes them touch.
const PILLS = new Set([
  'Badge', 'Button', 'Tag', 'Chip', 'AddButton', 'IconButton', 'Pill',
])
const HAS_GAP = /\b(gap-|gap-x-|space-x-|divide-x)/
const IS_FLEX = /\bflex\b/
const IS_COL = /\bflex-col\b/
// justify-{between,around,evenly} distribute free space between children, so
// adjacent pills do NOT touch even without a gap utility — not a zero-gap risk.
const DISTRIBUTES = /\bjustify-(between|around|evenly)\b/

function findFiles(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (
        !['node_modules', 'dist', 'build', '.git', 'tests'].includes(e) &&
        !full.endsWith(path.join('components', 'ui', 'kit')) &&
        !full.endsWith(path.join('components', 'ui', 'shadcn'))
      )
        findFiles(full, acc)
    } else if (/\.tsx$/.test(e) && !e.endsWith('.generated.ts')) acc.push(full)
  }
  return acc
}

const tagName = node => {
  const el = ts.isJsxElement(node)
    ? node.openingElement
    : ts.isJsxSelfClosingElement(node)
      ? node
      : null
  if (!el) return null
  const n = el.tagName
  return ts.isIdentifier(n) ? n.text : n.getText()
}

// Collect all string literals inside a className attribute value (handles
// `className="…"` and `className={cn('…', foo && '…')}`).
function classNameOf(node) {
  const el = ts.isJsxElement(node) ? node.openingElement : node
  const props = el.attributes?.properties ?? []
  for (const a of props) {
    if (!ts.isJsxAttribute(a) || a.name?.getText() !== 'className') continue
    const init = a.initializer
    if (!init) return ''
    let out = ''
    const collect = n => {
      if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) out += ' ' + n.text
      ts.forEachChild(n, collect)
    }
    collect(init)
    return out
  }
  return ''
}

function hasAttr(node, name) {
  const el = ts.isJsxElement(node) ? node.openingElement : node
  return (el.attributes?.properties ?? []).some(
    a => ts.isJsxAttribute(a) && a.name?.getText() === name,
  )
}

const violations = []
for (const root of ROOTS) {
  for (const file of findFiles(root)) {
    const sf = ts.createSourceFile(
      file,
      fs.readFileSync(file, 'utf-8'),
      ts.ScriptTarget.Latest,
      true,
      ts.ScriptKind.TSX,
    )
    const visit = node => {
      if (ts.isJsxElement(node)) {
        const cls = classNameOf(node)
        const isFlexRow = IS_FLEX.test(cls) && !IS_COL.test(cls)
        if (
          isFlexRow &&
          !HAS_GAP.test(cls) &&
          !DISTRIBUTES.test(cls) &&
          !hasAttr(node, OPT_OUT)
        ) {
          // consecutive pill children (ignore whitespace/expression text)
          let runStart = null
          let run = 0
          const kids = node.children.filter(
            c => ts.isJsxElement(c) || ts.isJsxSelfClosingElement(c),
          )
          for (const c of kids) {
            const t = tagName(c)
            if (t && PILLS.has(t)) {
              run++
              if (run === 1) runStart = c
              if (run === 2) {
                const { line, character } = sf.getLineAndCharacterOfPosition(
                  node.openingElement.getStart(sf),
                )
                violations.push({
                  file: sf.fileName,
                  line: line + 1,
                  col: character + 1,
                  msg: `flex-row container with adjacent <${tagName(runStart)}>+<${t}> and no gap-/space-x utility (zero-gap adjacency risk)`,
                })
                break
              }
            } else run = 0
          }
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(sf)
  }
}

if (violations.length) {
  console.error(
    `\n[adjacent-inline] ✗ ${violations.length} flex-row(s) with gap-less adjacent pills:\n`,
  )
  for (const v of violations)
    console.error(`  ${path.relative(process.cwd(), v.file)}:${v.line}:${v.col}  ${v.msg}`)
  console.error(
    `\nAdd a horizontal spacing utility (\`gap-2\`, \`gap-3\`, \`space-x-2\`) to the\n` +
      `container, OR — for a deliberately-joined group (segmented control, button\n` +
      `group where the buttons SHOULD touch) — add the \`${OPT_OUT}\` flag.\n`,
  )
  process.exit(1)
}
console.log('[adjacent-inline] ✓ no gap-less adjacent inline pills.')
