/**
 * Layer-3 lint (taxonomy N1, RTL-readiness): physical direction utilities in
 * NEW/CHANGED code. Tailwind physical-direction classes hard-code left/right and
 * therefore DON'T flip under `dir="rtl"`; their logical equivalents do:
 *
 *     pl-  → ps-        ml-  → ms-        left-       → start-
 *     pr-  → pe-        mr-  → me-        right-      → end-
 *     text-left → text-start            text-right   → text-end
 *
 * The goal is RTL-readiness by default: keep every NEW component RTL-clean so an
 * eventual i18n/RTL pass is a config flip, not a codebase rewrite. This is why the
 * lint is DIFF-SCOPED — it flags only lines ADDED on this branch (vs the merge-base
 * with origin/main), so the large backlog of pre-existing physical utilities in
 * untouched legacy code never fails the build, and touching a legacy file doesn't
 * suddenly punish its unrelated old lines. It is ACTIVE (exit 1 on a finding).
 *
 * Accuracy: only `className` string content is inspected (via the TS AST), so prose
 * / URLs / labels containing the word "left" are never false-flagged.
 *
 * Genuine physical needs (an animation/transform anchor, a deliberately LTR-locked
 * scrubber, an icon that must NOT mirror) opt out with an inline `rtl-ok` marker on
 * the same source line — put it in a trailing `//` or block comment next to the
 * className, e.g. a line ending with `// rtl-ok: keyframe anchor`.
 *
 * Sibling dormant taxonomy rows (documented in DEFECT_TAXONOMY.md §N, NOT wired —
 * they need a shipped RTL locale first):
 *   N2 [T]  `dir="rtl"` render matrix — render key surfaces RTL and diff vs LTR.
 *   N3 [V]  mirrored-crop vision review — icons/affordances that must flip vs must-not.
 *
 *   node scripts/lint-logical-direction.mjs
 */
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import { execFileSync } from 'node:child_process'
import path from 'node:path'
import fs from 'node:fs'

import { parseMulti } from './roots.mjs'

const require = createRequire(import.meta.url)
const ts = require('typescript')
// Parameterized over the app's src path(s) via `--path-include=<substr>` (repeatable):
// only changed files whose repo-relative path contains one of these substrings are
// scanned. Defaults to ziee's monorepo UI trees for backward-compatible behavior.
const PATH_INCLUDE = (() => {
  const p = parseMulti('path-include')
  return p.length ? p : ['src-app/ui/src/', 'src-app/desktop/ui/src/']
})()
const OPT_OUT = 'rtl-ok'

// Physical utility → logical replacement. The key regex matches the utility as a
// standalone class token: an optional variant chain (`sm:`, `hover:`, `dark:` …) and
// an optional `!` important / `-` negative prefix, with a value char after so we only
// match real utilities (`pl-4`, `-ml-1`, `left-1/2`, `text-left`), never a word.
const RULES = [
  { name: 'pl-', to: 'ps-', re: /(?<![\w-])!?-?pl-(?=[\w[.])/ },
  { name: 'pr-', to: 'pe-', re: /(?<![\w-])!?-?pr-(?=[\w[.])/ },
  { name: 'ml-', to: 'ms-', re: /(?<![\w-])!?-?ml-(?=[\w[.])/ },
  { name: 'mr-', to: 'me-', re: /(?<![\w-])!?-?mr-(?=[\w[.])/ },
  { name: 'left-', to: 'start-', re: /(?<![\w-])!?-?left-(?=[\w[.])/ },
  { name: 'right-', to: 'end-', re: /(?<![\w-])!?-?right-(?=[\w[.])/ },
  { name: 'text-left', to: 'text-start', re: /(?<![\w-])!?text-left(?![\w-])/ },
  { name: 'text-right', to: 'text-end', re: /(?<![\w-])!?text-right(?![\w-])/ },
]

// --- resolve the branch base + the set of added lines per file --------------------
function git(args, opts = {}) {
  return execFileSync('git', args, { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024, ...opts })
}
let repoRoot
try {
  repoRoot = git(['rev-parse', '--show-toplevel'], { stdio: ['ignore', 'pipe', 'ignore'] }).trim()
} catch {
  console.log('[logical-direction] not a git repo — skipping (nothing to diff).')
  process.exit(0)
}
function mergeBase() {
  for (const ref of ['origin/main', 'main']) {
    try {
      const b = git(['merge-base', 'HEAD', ref], { cwd: repoRoot, stdio: ['ignore', 'pipe', 'ignore'] }).trim()
      if (b) return b
    } catch {
      /* ref not present in this checkout */
    }
  }
  return null
}
const base = mergeBase()
if (!base) {
  console.log('[logical-direction] no origin/main|main base to diff against — skipping.')
  process.exit(0)
}

// `git diff --unified=0 <base>` = every change on this branch (committed + working
// tree) vs the fork point. Parse hunk headers to build file → Set(addedLineNos).
function isScanned(rel) {
  const p = rel.replace(/\\/g, '/')
  if (!/\.(tsx|ts)$/.test(p)) return false
  if (p.endsWith('.generated.ts') || p.endsWith('.d.ts')) return false
  return PATH_INCLUDE.some(inc => p.includes(inc))
}
let diff
try {
  diff = git(['diff', '--unified=0', '--no-color', base, '--', '*.tsx', '*.ts'], { cwd: repoRoot })
} catch {
  console.log('[logical-direction] git diff failed — skipping.')
  process.exit(0)
}
const added = new Map() // absFile → Set<number>
{
  let cur = null
  let newLine = 0
  for (const raw of diff.split('\n')) {
    if (raw.startsWith('+++ ')) {
      const rel = raw.slice(4).replace(/^b\//, '')
      cur = isScanned(rel) ? path.join(repoRoot, rel) : null
      if (cur && !added.has(cur)) added.set(cur, new Set())
      continue
    }
    if (raw.startsWith('@@')) {
      const m = /\+(\d+)(?:,(\d+))?/.exec(raw)
      newLine = m ? parseInt(m[1], 10) : 0
      continue
    }
    if (!cur) continue
    if (raw.startsWith('+') && !raw.startsWith('+++')) {
      added.get(cur).add(newLine)
      newLine++
    } else if (raw.startsWith('-') && !raw.startsWith('---')) {
      /* deletion: does not advance the new-file cursor */
    } else if (!raw.startsWith('\\')) {
      newLine++
    }
  }
}

// --- AST-scan each changed file; report className physical utils on ADDED lines ---
function collectClassNameNodes(sf) {
  // Return the className string-literal / template chunks (node w/ .text + position).
  const chunks = []
  const walk = node => {
    if (
      (ts.isJsxElement(node) || ts.isJsxSelfClosingElement(node))
    ) {
      const el = ts.isJsxElement(node) ? node.openingElement : node
      for (const a of el.attributes?.properties ?? []) {
        if (!ts.isJsxAttribute(a) || a.name?.getText() !== 'className') continue
        const collect = n => {
          if (ts.isStringLiteral(n) || ts.isNoSubstitutionTemplateLiteral(n)) chunks.push(n)
          else if (ts.isTemplateExpression(n)) {
            chunks.push(n.head, ...n.templateSpans.map(s => s.literal))
          }
          ts.forEachChild(n, collect)
        }
        if (a.initializer) collect(a.initializer)
      }
    }
    ts.forEachChild(node, walk)
  }
  walk(sf)
  return chunks
}

const findings = []
for (const [file, lines] of added) {
  if (!lines.size || !fs.existsSync(file)) continue
  const src = fs.readFileSync(file, 'utf-8')
  const srcLines = src.split('\n')
  const sf = ts.createSourceFile(file, src, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX)
  for (const node of collectClassNameNodes(sf)) {
    const text = node.text
    const textStart = node.getStart(sf) + 1 // skip opening quote/backtick
    for (const rule of RULES) {
      const g = new RegExp(rule.re, 'g')
      let m
      while ((m = g.exec(text))) {
        const { line } = sf.getLineAndCharacterOfPosition(textStart + m.index)
        const lineNo = line + 1
        if (!lines.has(lineNo)) continue
        if ((srcLines[line] ?? '').includes(OPT_OUT)) continue
        findings.push({ file, line: lineNo, token: m[0], from: rule.name, to: rule.to })
      }
    }
  }
}

findings.sort((a, b) => (a.file === b.file ? a.line - b.line : a.file < b.file ? -1 : 1))

if (findings.length) {
  console.log(
    `[logical-direction] ${findings.length} physical direction utilit${findings.length === 1 ? 'y' : 'ies'} in new/changed code — use the RTL-safe logical equivalent:\n`,
  )
  for (const f of findings.slice(0, 80)) {
    console.log(
      `  ${path.relative(process.cwd(), f.file)}:${f.line}  ${f.token.trim()}…  →  use \`${f.to}\``,
    )
  }
  if (findings.length > 80) console.log(`  … +${findings.length - 80} more`)
  console.log(
    `\nLogical props flip under dir="rtl": pl→ps pr→pe ml→ms mr→me left→start right→end` +
      ` text-left→text-start text-right→text-end.` +
      `\nFor a genuine physical need (transform/keyframe anchor, LTR-locked control) add an` +
      ` inline \`${OPT_OUT}\` marker on that line.`,
  )
  process.exit(1)
} else {
  console.log('[logical-direction] ✓ new/changed code uses logical direction utilities.')
}
