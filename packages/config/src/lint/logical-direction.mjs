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
// A SCOPE FLAG THAT IS SILENTLY IGNORED IS A GATE THAT SCANS THE WRONG TREE.
//
// Every other lint in this directory is scoped with `--root=<dir>` (roots.mjs),
// and callers reasonably pass the same flag here. This script read only
// `--path-include`, so `--root=src` was accepted, dropped on the floor, and the
// scan silently fell back to ziee's hard-coded trees. An app whose UI does NOT
// live at `src-app/ui/src/` therefore ran this lint over ZERO files and was told
// its new code was clean. `--root` is now honoured: each root is resolved
// against CWD and turned into the repo-relative prefix the diff filter wants.
const PATH_INCLUDE = (() => {
  const explicit = parseMulti('path-include')
  if (explicit.length) return explicit
  const roots = parseMulti('root')
  if (roots.length) {
    return roots.map(r => {
      const abs = path.resolve(process.cwd(), r)
      const rel = path.relative(repoRootEarly() ?? process.cwd(), abs).replace(/\\/g, '/')
      return rel ? (rel.endsWith('/') ? rel : rel + '/') : ''
    })
  }
  return ['src-app/ui/src/', 'src-app/desktop/ui/src/']
})()
function repoRootEarly() {
  try {
    return execFileSync('git', ['rev-parse', '--show-toplevel'], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
  } catch {
    return null
  }
}
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
// Explicit base, for a checkout where the default refs do not resolve (a shallow
// CI clone, a fork, a non-`main` trunk): `--base=<ref>` or $LINT_DIRECTION_BASE.
const BASE_OPT = (() => {
  const flag = process.argv.find(a => a.startsWith('--base='))
  return (flag ? flag.slice('--base='.length) : process.env.LINT_DIRECTION_BASE || '').trim()
})()
const ALLOW_NO_BASE = process.argv.includes('--allow-no-base')
const BASE_CANDIDATES = BASE_OPT
  ? [BASE_OPT]
  : ['origin/main', 'main', 'origin/HEAD', 'origin/master', 'master']

function mergeBase() {
  for (const ref of BASE_CANDIDATES) {
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
// A MISSING BASE IS A CONFIGURATION FAILURE, NOT A CLEAN RUN.
//
// This used to `console.log(...) ; process.exit(0)` — so on any checkout where
// neither ref resolved, the lint reported a friendly line and PASSED without
// examining a single file. That is not a rare state: `actions/checkout` defaults
// to `fetch-depth: 1`, which fetches ONE commit and NO branch refs, so
// `origin/main` does not exist and the lint was a guaranteed no-op in exactly
// the place it was meant to run. Nothing distinguished that from a real pass.
//
// It now refuses and names the fix. `--allow-no-base` still buys the old
// behaviour, but it has to be TYPED — an accidental skip and a declared one are
// no longer the same thing.
if (!base) {
  if (ALLOW_NO_BASE) {
    console.log('[logical-direction] no base ref — skipping (--allow-no-base was passed).')
    process.exit(0)
  }
  console.error(
    `[logical-direction] FAIL: no base ref to diff against.\n` +
      `  tried: ${BASE_CANDIDATES.join(', ')}\n` +
      `  This lint is diff-scoped; with no base it can see NOTHING, and reporting that\n` +
      `  as a pass is how it ran green over every branch in a shallow CI checkout.\n` +
      `  Fix (pick one):\n` +
      `    • CI: give the checkout history — actions/checkout with \`fetch-depth: 0\`,\n` +
      `      or fetch the trunk: \`git fetch --no-tags --depth=50 origin main:refs/remotes/origin/main\`\n` +
      `    • name the base explicitly: --base=<ref>  (or $LINT_DIRECTION_BASE)\n` +
      `    • genuinely no trunk to compare against: --allow-no-base (declares the skip)`,
  )
  process.exit(2)
}

// `git diff --unified=0 <base>` = every change on this branch (committed + working
// tree) vs the fork point. Parse hunk headers to build file → Set(addedLineNos).
// Files this run DECLINED because of the path scope (not the extension filter).
// Kept so a run that scanned nothing can say WHY — see the floor below.
const scopeExcluded = new Set()
function isScanned(rel) {
  const p = rel.replace(/\\/g, '/')
  if (!/\.(tsx|ts|css)$/.test(p)) return false
  if (p.endsWith('.generated.ts') || p.endsWith('.d.ts')) return false
  if (PATH_INCLUDE.some(inc => inc && p.includes(inc))) return true
  scopeExcluded.add(p)
  return false
}
let diff
try {
  diff = git(['diff', '--unified=0', '--no-color', base, '--', '*.tsx', '*.ts', '*.css'], { cwd: repoRoot })
} catch {
  console.error('[logical-direction] FAIL: git diff against ' + base + ' failed — the lint saw no input.')
  process.exit(2)
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

// UNTRACKED FILES ARE NEW CODE — `git diff <base>` never mentions them.
//
// The whole point of this lint is NEW components, and a brand-new component
// file is untracked until someone runs `git add`. Until then every one of its
// lines was invisible here: an agent could write a fresh `pl-4 text-left`
// component, run the lint, and be told its new code uses logical utilities.
// (Observed exactly that way.) Every line of an untracked file is an added
// line, so they are folded into the same map.
try {
  const others = git(['ls-files', '--others', '--exclude-standard', '-z'], { cwd: repoRoot })
  for (const rel of others.split('\0')) {
    if (!rel || !isScanned(rel)) continue
    const abs = path.join(repoRoot, rel)
    let count = 0
    try {
      count = fs.readFileSync(abs, 'utf-8').split('\n').length
    } catch {
      continue
    }
    const set = added.get(abs) ?? new Set()
    for (let i = 1; i <= count; i++) set.add(i)
    added.set(abs, set)
  }
} catch {
  /* ls-files is not load-bearing enough to fail the run; the diff arm still gated */
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

// CSS carries the same Tailwind utilities through `@apply`, and the same
// physical intent through raw longhand properties. The TSX arm cannot see
// either: it walks a TypeScript AST for `className` attributes, so a `.css`
// file was skipped whole. Nocturne's own sheets are `.css`.
const CSS_PROP_RULES = [
  { name: 'padding-left', to: 'padding-inline-start', re: /(?<![\w-])padding-left\s*:/ },
  { name: 'padding-right', to: 'padding-inline-end', re: /(?<![\w-])padding-right\s*:/ },
  { name: 'margin-left', to: 'margin-inline-start', re: /(?<![\w-])margin-left\s*:/ },
  { name: 'margin-right', to: 'margin-inline-end', re: /(?<![\w-])margin-right\s*:/ },
  { name: 'border-left', to: 'border-inline-start', re: /(?<![\w-])border-left(-[a-z]+)?\s*:/ },
  { name: 'border-right', to: 'border-inline-end', re: /(?<![\w-])border-right(-[a-z]+)?\s*:/ },
  { name: 'text-align: left', to: 'text-align: start', re: /text-align\s*:\s*left\b/ },
  { name: 'text-align: right', to: 'text-align: end', re: /text-align\s*:\s*right\b/ },
]

function scanCss(file, lines, findings) {
  const srcLines = fs.readFileSync(file, 'utf-8').split('\n')
  for (const lineNo of [...lines].sort((a, b) => a - b)) {
    const raw = srcLines[lineNo - 1]
    if (raw === undefined) continue
    if (raw.includes(OPT_OUT)) continue
    // Drop `/* … */` content so a comment mentioning `pl-4` is not a finding.
    const text = raw.replace(/\/\*[^]*?\*\//g, ' ')
    for (const rule of [...RULES, ...CSS_PROP_RULES]) {
      const m = new RegExp(rule.re).exec(text)
      if (m) findings.push({ file, line: lineNo, token: m[0], from: rule.name, to: rule.to })
    }
  }
}

const findings = []
let scannedFiles = 0
let scannedLines = 0
for (const [file, lines] of added) {
  if (!lines.size || !fs.existsSync(file)) continue
  scannedFiles++
  scannedLines += lines.size
  if (file.endsWith('.css')) {
    scanCss(file, lines, findings)
    continue
  }
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
    `[logical-direction] ${findings.length} physical direction utilit${findings.length === 1 ? 'y' : 'ies'} in new/changed code (${scannedFiles} file(s), ${scannedLines} added line(s) scanned vs ${base.slice(0, 9)}) — use the RTL-safe logical equivalent:\n`,
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
  // A RUN THAT SCANNED NOTHING MUST SAY SO — it is not a pass.
  //
  // Two of this lint's three original blind spots ended in the same place: the
  // script printed "✓ new/changed code uses logical direction utilities" having
  // opened zero files. That line is indistinguishable from a real pass in a CI
  // log, which is why nobody noticed. Every run now states its base, its scope,
  // and what it actually read.
  //
  // Scanning zero files is legitimate (a Rust-only branch). Scanning zero files
  // while the diff DID touch .ts/.tsx/.css outside the configured scope is not:
  // that is the scope being wrong, and it is the shape `--root=src` being
  // silently ignored used to produce.
  if (scannedFiles === 0 && scopeExcluded.size) {
    console.error(
      `[logical-direction] FAIL: scanned 0 files, but ${scopeExcluded.size} changed ` +
        `source file(s) were excluded BY SCOPE.\n` +
        `  base:  ${base}\n` +
        `  scope: ${PATH_INCLUDE.map(x => x || '(empty)').join(', ')}\n` +
        `  e.g.:  ${[...scopeExcluded].slice(0, 5).join('\n         ')}\n` +
        `  A run that opens no files is not a pass. Point the scope at this app's UI\n` +
        `  tree with --root=<dir> (or --path-include=<repo-relative-prefix>).`,
    )
    process.exit(2)
  }
  console.log(
    `[logical-direction] ✓ ${scannedFiles} changed file(s) / ${scannedLines} added line(s) ` +
      `use logical direction utilities  [base ${base.slice(0, 9)}, scope ${PATH_INCLUDE.map(x => x || '(repo root)').join(' ')}]`,
  )
}
