/**
 * OVERLAY-RENDER GATE — enumerate every overlay-bearing surface (Dialog / Drawer
 * / Sheet / Modal / Popover / AlertDialog / Confirm / Popconfirm) under
 * src/modules + src/components/ui, classify each, and enforce that every one is
 * EITHER rendered OPEN in the gallery (an OVERLAY_ENTRIES entry) OR carries an
 * explicit allow-list reason.
 *
 * ROOT PROBLEM this closes: overlays (edit drawers, the Skills-in-conversation
 * dialog, import modals, menus, popovers) were NEVER rendered open in the
 * gallery, so NO geometry / affordance / runtime / vision audit ever looked at
 * them. Cataloguing findings didn't help because the buggy states aren't on
 * screen. This gate makes an un-rendered overlay a BUILD FAILURE — the overlay
 * analog of the state-matrix render gate (gen-state-matrix.mjs) and the seeded
 * coverage gate (gen-gallery-coverage.mjs).
 *
 * An overlay occurrence is classified as:
 *   - `controlled` (HOST): the primitive has an externally-controlled open prop
 *     (`open=` / `open` shorthand / `isOpen` / `visible=`). The file's job is to
 *     render an overlay whose visibility a store/prop drives → it MUST be
 *     rendered OPEN in the gallery, or allow-listed.
 *   - `trigger` (SELF-OPENING): a Confirm / Popconfirm / Popover / Menu that
 *     wraps a trigger child and opens on user interaction (no open prop). It is
 *     reached by wiring its PARENT surface open + an interaction recipe → it
 *     needs a `triggers` allow-list entry documenting where it opens (or the
 *     parent wired).
 *
 * Surface id = path from src/ without extension (matches coverage.ts ids), e.g.
 *   modules/skill/components/SkillConversationDrawer
 *
 * Config-driven (`@ziee/gallery`): src root (`srcDir`), walked roots
 * (`surfaceRoots`), gallery dir (`galleryDir`), and the overlay-primitive import
 * specifiers (`overlayKitImports`) come from the app's `gallery.config.json`.
 *
 * Run: node <pkg>/scripts/gen-overlay-registry.mjs           (write the registry JSON)
 *      node <pkg>/scripts/gen-overlay-registry.mjs --check    (gate: fail on un-rendered)
 *      node <pkg>/scripts/gen-overlay-registry.mjs --list      (human summary to stdout)
 */
import { pathToFileURL } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

const CFG = resolveGalleryConfig()
const CWD = CFG.__cwd
const SRC = path.resolve(CWD, CFG.srcDir)
const GALLERY = path.resolve(CWD, CFG.galleryDir)
const OVERLAYS_TS = path.join(GALLERY, 'overlays.tsx')
const ALLOWLIST = path.join(GALLERY, 'overlay-allowlist.json')
const OUT = path.join(GALLERY, 'overlay-registry.generated.json')
const MODULES_DIR = path.resolve(CWD, CFG.surfaceRoots[0])

const PRIMITIVES = [
  'Dialog',
  'Drawer',
  'Sheet',
  'Modal',
  'Popover',
  'AlertDialog',
  'Confirm',
  'Popconfirm',
]

// The kit modules an overlay primitive is legitimately imported from. A local
// component that happens to share a name (imported from elsewhere) is NOT an
// overlay primitive and must not trip the gate. Driven by `overlayKitImports`:
// each entry P matches `source === P || source.startsWith(P + '/')`.
const isKitSource = source =>
  CFG.overlayKitImports.some(p => source === p || source.startsWith(`${p}/`))

const ROOTS = CFG.surfaceRoots.map((rel, i) => ({
  dir: path.resolve(CWD, rel),
  skip: i === 0 ? f => f === 'module.tsx' : () => false,
}))

function walk(dir, acc = []) {
  if (!fs.existsSync(dir)) return acc
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    const st = fs.statSync(full)
    if (st.isDirectory()) {
      if (!['node_modules', 'dist', 'build', '.git', 'tests', '__tests__'].includes(e))
        walk(full, acc)
    } else if (/\.tsx$/.test(e) && !/\.(test|stories)\.tsx$/.test(e)) {
      acc.push(full)
    }
  }
  return acc
}

/** The set of primitive names actually imported from the kit in this file. */
function importedPrimitives(src) {
  const found = new Set()
  // Scan every import statement that resolves to a kit module; collect the named
  // bindings that are overlay primitives.
  const importRe = /import\s+(?:type\s+)?\{([^}]*)\}\s+from\s+'([^']+)'/g
  let m
  while ((m = importRe.exec(src))) {
    const [, names, source] = m
    if (!isKitSource(source)) continue
    for (const raw of names.split(',')) {
      const name = raw.trim().split(/\s+as\s+/)[0].trim()
      if (PRIMITIVES.includes(name)) found.add(name)
    }
  }
  return found
}

/**
 * Find each overlay-primitive JSX occurrence and classify it. Returns
 * [{ primitive, mode: 'controlled' | 'trigger' }].
 */
function scanOccurrences(src, imported) {
  const out = []
  for (const prim of PRIMITIVES) {
    if (!imported.has(prim)) continue
    // Match the opening tag and grab attributes up to the closing '>' of the tag
    // (naive: stops at first '>', which is fine for detecting the open-prop that
    // conventionally sits at the top of the tag).
    const tagRe = new RegExp(`<${prim}\\b([^>]*)>`, 'gs')
    let m
    while ((m = tagRe.exec(src))) {
      const attrs = m[1]
      const controlled =
        /\bopen\s*=/.test(attrs) ||
        /\bopen\b(?!\w)/.test(attrs) || // `open` shorthand
        /\bisOpen\s*=/.test(attrs) ||
        /\bvisible\s*=/.test(attrs)
      out.push({ primitive: prim, mode: controlled ? 'controlled' : 'trigger' })
    }
    // Self-closing form `<Prim ... />` (e.g. a bare <Drawer open .../>).
    const selfRe = new RegExp(`<${prim}\\b([^>]*)/>`, 'gs')
    while ((m = selfRe.exec(src))) {
      const attrs = m[1]
      const controlled =
        /\bopen\s*=/.test(attrs) ||
        /\bopen\b(?!\w)/.test(attrs) ||
        /\bisOpen\s*=/.test(attrs) ||
        /\bvisible\s*=/.test(attrs)
      // Avoid double-counting: the `<...>` regex above also matches self-closing
      // tags' leading `<Prim ...>` only if there's a `>`; self-closing ends in
      // `/>` so it's NOT matched by tagRe (which requires the char before `>` to
      // not be part of `/>`? actually it does match). De-dupe by attrs identity.
      if (!out.some(o => o.primitive === prim && o._attrs === attrs))
        out.push({ primitive: prim, mode: controlled ? 'controlled' : 'trigger', _attrs: attrs })
    }
  }
  return out.map(({ primitive, mode }) => ({ primitive, mode }))
}

function collect() {
  const surfaces = []
  for (const { dir, skip } of ROOTS) {
    for (const f of walk(dir)) {
      if (skip(path.basename(f))) continue
      const src = fs.readFileSync(f, 'utf-8')
      const imported = importedPrimitives(src)
      if (!imported.size) continue
      const occ = scanOccurrences(src, imported)
      if (!occ.length) continue
      const rel = path.relative(SRC, f).replace(/\\/g, '/').replace(/\.tsx$/, '')
      const hasControlled = occ.some(o => o.mode === 'controlled')
      const primitives = [...new Set(occ.map(o => o.primitive))].sort()
      surfaces.push({
        surface: rel,
        class: hasControlled ? 'host' : 'trigger',
        primitives,
        occurrences: occ,
      })
    }
  }
  return surfaces.sort((a, b) => a.surface.localeCompare(b.surface))
}

/** Surfaces wired OPEN as gallery overlay entries (regex the `surface:` fields).
 *  Overlay entries are OWNED per-module in `src/modules/<X>/gallery.tsx`
 *  (`gallery.overlays`), auto-discovered by the gallery's runtime registry — so
 *  scan every module `gallery.tsx` (plus the residual central `overlays.tsx` for
 *  back-compat). Reading only `overlays.tsx` (now a thin aggregator with no
 *  `surface:` fields) would false-fail every host overlay. */
/** Pure: extract every `surface: '…'`/`surface: "…"` id from source texts (TEST-5). */
export function extractWiredSurfaces(srcTexts) {
  const set = new Set()
  const re = /surface:\s*['"]([^'"]+)['"]/g
  for (const src of srcTexts) {
    let m
    re.lastIndex = 0
    while ((m = re.exec(src))) set.add(m[1])
  }
  return set
}

function wiredSurfaces() {
  const files = []
  if (fs.existsSync(OVERLAYS_TS)) files.push(OVERLAYS_TS)
  if (fs.existsSync(MODULES_DIR)) {
    for (const m of fs.readdirSync(MODULES_DIR)) {
      for (const name of ['gallery.tsx', 'gallery.ts']) {
        const p = path.join(MODULES_DIR, m, name)
        if (fs.existsSync(p)) files.push(p)
      }
    }
  }
  return extractWiredSurfaces(files.map(f => fs.readFileSync(f, 'utf-8')))
}

function loadAllowlist() {
  if (!fs.existsSync(ALLOWLIST)) return { hosts: {}, triggers: {} }
  const j = JSON.parse(fs.readFileSync(ALLOWLIST, 'utf-8'))
  return { hosts: j.hosts ?? {}, triggers: j.triggers ?? {} }
}

// Portable main-module check (the naive `file://${argv[1]}` is false on Windows
// + on spaced paths, silently disabling the gate).
const isMain = import.meta.url === pathToFileURL(process.argv[1]).href
if (isMain) {
const surfaces = collect()
const wired = wiredSurfaces()
const allow = loadAllowlist()

const hosts = surfaces.filter(s => s.class === 'host')
const triggers = surfaces.filter(s => s.class === 'trigger')

const registry = {
  generatedBy: 'scripts/gen-overlay-registry.mjs',
  counts: {
    total: surfaces.length,
    hosts: hosts.length,
    triggers: triggers.length,
    wiredOpen: hosts.filter(s => wired.has(s.surface)).length,
  },
  hosts,
  triggers,
}

const mode = process.argv.includes('--check')
  ? 'check'
  : process.argv.includes('--list')
    ? 'list'
    : 'write'

function statusOf(s) {
  if (wired.has(s.surface)) return 'wired'
  const bucket = s.class === 'host' ? allow.hosts : allow.triggers
  if (bucket[s.surface]) return 'allowlisted'
  return 'MISSING'
}

if (mode === 'write') {
  fs.writeFileSync(OUT, `${JSON.stringify(registry, null, 2)}\n`)
  console.log(
    `Wrote ${path.relative(SRC, OUT)} — ${surfaces.length} overlay surfaces ` +
      `(${hosts.length} hosts, ${triggers.length} triggers; ${registry.counts.wiredOpen} hosts wired open).`,
  )
} else if (mode === 'list') {
  for (const cls of ['host', 'trigger']) {
    const list = surfaces.filter(s => s.class === cls)
    console.log(`\n=== ${cls.toUpperCase()}S (${list.length}) ===`)
    for (const s of list) {
      console.log(`  [${statusOf(s).padEnd(11)}] ${s.surface}  {${s.primitives.join(',')}}`)
    }
  }
} else {
  // check: drift-guard the generated registry + fail on any MISSING surface.
  const cur = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf-8') : ''
  if (cur.trim() !== JSON.stringify(registry, null, 2).trim()) {
    console.error(
      'overlay-registry.generated.json is stale — run `node scripts/gen-overlay-registry.mjs` and commit.',
    )
    process.exit(1)
  }
  const missingHosts = hosts.filter(s => statusOf(s) === 'MISSING')
  const missingTriggers = triggers.filter(s => statusOf(s) === 'MISSING')
  // Stale allow-list entries (a surface removed / now wired): keep the list honest.
  const staleAllow = [
    ...Object.keys(allow.hosts).filter(
      k => wired.has(k) || !hosts.some(s => s.surface === k),
    ),
    ...Object.keys(allow.triggers).filter(
      k => wired.has(k) || !triggers.some(s => s.surface === k),
    ),
  ]

  if (missingHosts.length || missingTriggers.length || staleAllow.length) {
    if (missingHosts.length) {
      console.error(
        `\n${missingHosts.length} overlay HOST(s) are never rendered open in the gallery.`,
      )
      console.error(
        'Add an OVERLAY_ENTRIES entry (src/dev/gallery/overlays.tsx) that fires the ' +
          'store/prop open action, OR add an allow-list reason to ' +
          'src/dev/gallery/overlay-allowlist.json ("hosts").',
      )
      for (const s of missingHosts)
        console.error(`  ✗ ${s.surface}  {${s.primitives.join(',')}}`)
    }
    if (missingTriggers.length) {
      console.error(
        `\n${missingTriggers.length} self-opening overlay(s) (Confirm/Popover/menu) have no ` +
          'render coverage. Wire the parent open + an interaction recipe, OR add an ' +
          'allow-list reason to overlay-allowlist.json ("triggers").',
      )
      for (const s of missingTriggers)
        console.error(`  ✗ ${s.surface}  {${s.primitives.join(',')}}`)
    }
    if (staleAllow.length) {
      console.error(
        `\n${staleAllow.length} stale allow-list entry/entries (now wired or gone) — remove from overlay-allowlist.json:`,
      )
      for (const k of staleAllow) console.error(`  ✗ ${k}`)
    }
    process.exit(1)
  }
  console.log(
    `overlay gate OK — ${surfaces.length} overlay surfaces ` +
      `(${hosts.length} hosts, ${triggers.length} triggers): ` +
      `${registry.counts.wiredOpen} wired open, ` +
      `${surfaces.length - registry.counts.wiredOpen} allow-listed.`,
  )
}
}
