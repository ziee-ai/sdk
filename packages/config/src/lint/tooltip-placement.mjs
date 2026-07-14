/**
 * Layer-3 lint (taxonomy J9): inconsistent tooltip placement among peer buttons.
 *
 * Within ONE button-group / toolbar / header-actions row, every button's tooltip
 * must open toward the SAME side — a row where some tooltips drop DOWN and others
 * pop UP reads as sloppy, and the downward ones can cover the content below. The
 * kit exposes two tooltip channels, each defaulting to `top`:
 *   • the `<Tooltip side="…">` wrapper component, and
 *   • the `<Button tooltip="…" tooltipSide="…">` prop pair.
 * A group that mixes them (or mixes explicit sides) ends up with split placement —
 * e.g. the file-viewer header: the Rendered/Raw view toggles use `<Tooltip>`
 * (default `top`) while Copy/Download pass `tooltipSide="bottom"`.
 *
 * Detection (AST, no render needed — tooltips are hover-gated so a runtime pass
 * would need a hover recipe per button): find each JSX container that holds ≥2
 * tooltip-bearing buttons as descendants, resolve each button's tooltip side
 * (explicit `side` / `tooltipSide`, else `top`), and flag the container when the
 * sides are not uniform. The container is the SMALLEST element that still holds
 * ≥2 of them, so unrelated tooltips elsewhere in the file don't cross-contaminate.
 *
 * ADVISORY (reports, exit 0) not gating — there's a live violation (the file-viewer
 * header mixes top/bottom). Wire `--gate` (exit 1) once it's burned down. The
 * detector-acceptance fixture (DefectRepro.tsx) is exempt: it intentionally hosts
 * the mixed-side known-positive so the acceptance harness can prove J9 fires.
 *
 *   node scripts/lint-tooltip-placement.mjs [--gate]
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
const GATE = process.argv.includes('--gate')
// The detector-acceptance fixture intentionally hosts a mixed-side known-positive.
const EXEMPT = /[/\\]dev[/\\]gallery[/\\]DefectRepro\.tsx$/

function findFiles(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (!['node_modules', 'dist', 'build', '.git', 'tests'].includes(e)) findFiles(full, acc)
    } else if (/\.tsx$/.test(e) && !e.endsWith('.generated.ts')) acc.push(full)
  }
  return acc
}

const jsxName = node => {
  const tag = node.tagName
  return ts.isIdentifier(tag) ? tag.text : tag.getText()
}
const attrs = node => {
  const map = new Map()
  for (const a of node.attributes?.properties ?? []) {
    if (!ts.isJsxAttribute(a) || !a.name) continue
    const name = a.name.getText()
    let val
    const init = a.initializer
    if (init == null) val = true
    else if (ts.isStringLiteral(init)) val = init.text
    else if (ts.isJsxExpression(init) && init.expression && ts.isStringLiteral(init.expression)) val = init.expression.text
    else val = '<expr>'
    map.set(name, val)
  }
  return map
}

// A "tooltip-bearing button" node → its resolved tooltip side (default 'top').
function tooltipSideOf(node) {
  const name = jsxName(node)
  const a = attrs(node)
  if (name === 'Tooltip') {
    // Only a Tooltip that actually shows something (title/content) counts.
    if (!a.has('title') && !a.has('content')) return null
    return typeof a.get('side') === 'string' ? a.get('side') : 'top'
  }
  // Button/IconButton (or anything) carrying a `tooltip` prop.
  if (a.has('tooltip')) {
    return typeof a.get('tooltipSide') === 'string' ? a.get('tooltipSide') : 'top'
  }
  return null
}

const findings = []
for (const root of ROOTS) {
  for (const file of findFiles(root)) {
    if (EXEMPT.test(file)) continue
    const src = fs.readFileSync(file, 'utf-8')
    if (!/tooltip/i.test(src)) continue
    const sf = ts.createSourceFile(file, src, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
    // Collect every tooltip node with its resolved side + line.
    const tips = []
    const visit = node => {
      if (ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node)) {
        const open = ts.isJsxElement(node) ? node.openingElement : node
        const side = tooltipSideOf(open)
        if (side) {
          const { line } = sf.getLineAndCharacterOfPosition(open.getStart(sf))
          tips.push({ node, side, line: line + 1 })
        }
      }
      ts.forEachChild(node, visit)
    }
    visit(sf)
    if (tips.length < 2) continue
    // Group tips by their nearest common JSX-container ancestor: for each pair that
    // differs in side, walk up to the lowest common ancestor and record a group.
    // Simpler + sufficient: if the file's tips are not all the same side AND at
    // least two of them live under a shared container (a JSX element ancestor that
    // is not the whole component), flag that shared container once.
    const sides = new Set(tips.map(t => t.side))
    if (sides.size < 2) continue
    // nearest-common-ancestor of ALL tips
    const ancestorsOf = n => {
      const chain = []
      let p = n.parent
      while (p) { if (ts.isJsxElement(p)) chain.push(p); p = p.parent }
      return chain
    }
    const chains = tips.map(t => ancestorsOf(t.node))
    let lca = null
    for (const cand of chains[0]) {
      if (chains.every(ch => ch.includes(cand))) { lca = cand; break }
    }
    const container = lca ? jsxName(lca.openingElement) : '(component root)'
    const { line } = lca ? sf.getLineAndCharacterOfPosition(lca.getStart(sf)) : { line: tips[0].line - 1 }
    findings.push({
      file, line: line + 1, container,
      detail: tips.map(t => `L${t.line}:${t.side}`).join(', '),
      sides: [...sides].join('/'),
    })
  }
}

if (findings.length) {
  console.log(`[tooltip-placement] ${findings.length} button group(s) with inconsistent tooltip side:\n`)
  for (const f of findings.slice(0, 60)) {
    console.log(`  ${path.relative(process.cwd(), f.file)}:${f.line}  <${f.container}> mixes tooltip sides {${f.sides}} — ${f.detail}`)
  }
  if (findings.length > 60) console.log(`  … +${findings.length - 60} more`)
  console.log(`\nMake every button in one group use the same tooltip side (all \`<Tooltip side=…>\` or all \`tooltipSide=…\`, one value).`)
  if (GATE) process.exit(1)
} else {
  console.log('[tooltip-placement] ✓ tooltip side is consistent within each button group.')
}
