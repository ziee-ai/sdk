/**
 * Prefetch-coverage gate for the unified lazy-store pattern.
 *
 * In the pattern, every store is a folder and every action is its own file under
 * `stores/<store>/actions/<name>.ts`, loaded as its own lazy chunk on first call
 * (or `.preload()`). To keep clicks instant, an action's chunk must be *prefetched*
 * ahead of the click — the convention is `onMouseEnter/onFocus` on the triggering
 * control calling `<Store>.<action>.preload()`.
 *
 * This gate FORCES that wiring: for every action file it requires at least one of
 *   (a) an explicit prefetch — `.<action>.preload(` anywhere in the app source, OR
 *   (b) a load-on-mount invocation in the store's own entry `init` —
 *       `actions.<action>(` in `stores/<store>/index.ts` (already warmed on first
 *       access, so no separate prefetch is needed).
 * An action satisfying NEITHER fails the build with an actionable message. There
 * is deliberately no exempt marker: a genuinely programmatic action warms itself
 * via `actions.<action>.preload()` in `init` (which matches (a)), so the strict
 * rule stays satisfiable without a loophole an agent could paper over.
 *
 * Detection is textual (regex), not AST: it only needs to know an action file
 * exists and whether a `.preload(`/init-invocation for its name appears. The one
 * known imprecision — two DIFFERENT stores with an identically-named action —
 * can let one store's wiring satisfy the other. That errs toward a false PASS
 * (never a false FAIL that blocks valid code); action names here are descriptive
 * enough that it effectively never happens. Bind by handle later if it does.
 *
 *   node action-prefetch.mjs --root=src --root=../desktop/ui/src
 */
import path from 'node:path'
import fs from 'node:fs'

import { parseRoots } from './roots.mjs'

const ROOTS = parseRoots()
const SKIP_DIRS = ['node_modules', 'dist', 'build', '.git']
const IS_TEST = f => /\.(test|spec)\.[tj]sx?$/.test(f)

/** Recursively collect files under `dir` matching `pred(fullPath, name)`. */
function walk(dir, pred, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (!SKIP_DIRS.includes(e)) walk(full, pred, acc)
    } else if (pred(full, e)) acc.push(full)
  }
  return acc
}

/** A file is an ACTION file when its immediate parent dir is `actions` and some
 *  ancestor segment is `stores` (so unrelated `actions/` dirs aren't swept in). */
function isActionFile(full, name) {
  if (!/\.ts$/.test(name) || IS_TEST(name)) return false
  const parts = full.split(path.sep)
  const parent = parts[parts.length - 2]
  return parent === 'actions' && parts.includes('stores')
}

// 1. Enumerate action files + build the searchable source corpus in one walk.
const actions = [] // { name, file, storeDir }
const corpus = [] // { file, text }  (all non-test ts/tsx across roots)
for (const root of ROOTS) {
  for (const full of walk(root, (f, n) => /\.(ts|tsx)$/.test(n) && !IS_TEST(n))) {
    corpus.push({ file: full, text: fs.readFileSync(full, 'utf8') })
    if (isActionFile(full, path.basename(full))) {
      const name = path.basename(full, '.ts')
      // storeDir = the `<store>` folder that owns `actions/` (parent of actions dir)
      const storeDir = path.dirname(path.dirname(full))
      actions.push({ name, file: full, storeDir })
    }
  }
}

const esc = s => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')

/** Entry text for a store folder. The unified layout is `<storeDir>/index.ts`;
 *  an older layout keeps the store file beside the actions folder
 *  (`stores/<Name>.store.ts` next to `stores/<name>/actions/`). Check, in order:
 *  `<storeDir>/index.ts`, a `*.store.ts` INSIDE `<storeDir>`, and a sibling
 *  `*.store.ts` in the PARENT whose name matches the folder (case-insensitive). */
function entryText(storeDir) {
  const idx = path.join(storeDir, 'index.ts')
  if (fs.existsSync(idx)) return fs.readFileSync(idx, 'utf8')
  const inside = fs.readdirSync(storeDir).find(f => /\.store\.ts$/.test(f))
  if (inside) return fs.readFileSync(path.join(storeDir, inside), 'utf8')
  const parent = path.dirname(storeDir)
  const folder = path.basename(storeDir).toLowerCase()
  const sibling = fs
    .readdirSync(parent)
    .find(
      f => /\.store\.ts$/.test(f) && f.replace(/\.store\.ts$/, '').toLowerCase() === folder,
    )
  return sibling ? fs.readFileSync(path.join(parent, sibling), 'utf8') : ''
}

// 2. Check each action for a prefetch site or an init invocation.
const unwired = []
for (const a of actions) {
  const preloadRe = new RegExp(`\\.${esc(a.name)}\\.preload\\s*\\(`)
  const hasPreload = corpus.some(
    c => c.file !== a.file && preloadRe.test(c.text),
  )
  const initInvokeRe = new RegExp(`\\bactions\\.${esc(a.name)}\\s*\\(`)
  const invokedInInit = initInvokeRe.test(entryText(a.storeDir))
  if (!hasPreload && !invokedInInit) unwired.push(a)
}

if (unwired.length === 0) {
  console.log(
    `✓ action-prefetch: all ${actions.length} lazy actions are prefetched or init-invoked`,
  )
  process.exit(0)
}

console.error(
  `\n✗ action-prefetch: ${unwired.length} lazy action(s) have no prefetch wiring:\n`,
)
for (const a of unwired) {
  console.error(`  ${path.relative(process.cwd(), a.file)}`)
  console.error(
    `    → wire \`<Store>.${a.name}.preload()\` on the trigger's onMouseEnter/onFocus,`,
  )
  console.error(
    `      or invoke \`actions.${a.name}()\` in the store's init (load-on-mount).\n`,
  )
}
console.error(
  'Every lazy action must have its chunk prefetched on intent (hover/focus) so the\n' +
    'click is instant — or be a load-on-mount action invoked in init. See the\n' +
    'unified lazy-store pattern.\n',
)
process.exit(1)
