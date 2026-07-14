/**
 * Generate GALLERY_SEED_MANIFEST.md — the living per-module index of dev-gallery
 * seed ownership — and gate that every surface-bearing module OWNS a
 * `src/modules/<X>/gallery.tsx`.
 *
 * Mirrors gen-override-registry.mjs: a set-difference gate whose inputs are all
 * committed product-tree paths (never `.lifecycle/`, which is stripped at merge),
 * a byte-compared generated manifest, and pure exported functions for unit tests.
 *
 * MISSING = { module with a user-facing surface AND no gallery.tsx AND not
 *             allow-listed } → exit 1.
 * STALE   = { allow-listed module that now HAS a gallery.tsx OR has no surface }
 *             → exit 1 (GC — the excuse list can't rot).
 *
 * "User-facing surface" (plain Node, no vite): the module's module.tsx declares a
 * route `path:` not in the skip-set, OR registers a user-facing slot key.
 *
 * Run: node scripts/gen-gallery-seed-registry.mjs        (write manifest)
 *      node scripts/gen-gallery-seed-registry.mjs --check (drift + missing-seed gate)
 */
import { pathToFileURL } from 'node:url'
import path from 'node:path'
import fs from 'node:fs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

// The workspace `src` dir to gate. Defaults to `srcDir` from the app's
// `gallery.config.json` (resolved against the invoking cwd); the desktop
// workspace runs the SAME script over its own tree via `--src <dir>` (a portable
// arg — an inline `VAR=` env prefix is not Windows-npm portable). Keeps ONE gate,
// no copy/drift.
const CFG = resolveGalleryConfig()
const srcArgIdx = process.argv.indexOf('--src')
const SRC =
  srcArgIdx !== -1 && process.argv[srcArgIdx + 1]
    ? path.resolve(CFG.__cwd, process.argv[srcArgIdx + 1])
    : path.resolve(CFG.__cwd, CFG.srcDir)
const MODULES_DIR = path.join(SRC, 'modules')
const OUT = path.join(SRC, 'dev/gallery/GALLERY_SEED_MANIFEST.md')
const EXCEPTIONS_PATH = path.join(SRC, 'dev/gallery/GALLERY_SEED_EXCEPTIONS.md')

/** Routes that are not reviewable page CONTENT (mirrors pages.tsx SKIP_PATHS). */
const SKIP_PATHS = new Set(['/', '/dev/gallery', '/auth/callback'])

/** Slot keys whose registration means the module renders user-facing UI. */
const USER_SLOT_KEYS = [
  'settingsUserPages',
  'settingsAdminPages',
  'sidebarNavigation',
  'sidebarContent',
  'sidebarBottom',
  'sidebarFooter',
  'sidebarTools',
  'sidebarPrimaryActions',
  'appBanners',
  'registerPanelRenderer',
]

/** Strip block + line comments so a COMMENTED-OUT `path:`/slot (e.g. a disabled
 *  route) isn't miscounted as a live surface. Naive but safe here. */
function stripComments(src) {
  return src
    .replace(/\/\*[\s\S]*?\*\//g, '')
    .replace(/(^|[^:])\/\/[^\n]*/g, '$1')
}

/**
 * Does this module.tsx source declare a user-facing surface? True if it declares
 * a route `path:` literal not in the skip-set, OR references a user-facing slot
 * key. Comments are stripped first. Pure — exported for TEST-3.
 */
export function hasUserSurface(moduleSrcRaw) {
  const moduleSrc = stripComments(moduleSrcRaw)
  const paths = [...moduleSrc.matchAll(/path:\s*['"]([^'"]+)['"]/g)].map(m => m[1])
  const hasReviewableRoute = paths.some(p => !SKIP_PATHS.has(p))
  const hasSlot = USER_SLOT_KEYS.some(k => new RegExp(`\\b${k}\\b`).test(moduleSrc))
  return hasReviewableRoute || hasSlot
}

/** Does the module dir carry a `gallery.{ts,tsx}` that exports `gallery`? */
export function hasSeed(moduleDir) {
  for (const name of ['gallery.tsx', 'gallery.ts']) {
    const p = path.join(moduleDir, name)
    if (fs.existsSync(p) && /export\s+const\s+gallery\b/.test(fs.readFileSync(p, 'utf-8')))
      return true
  }
  return false
}

/** Approved no-seed exceptions from GALLERY_SEED_EXCEPTIONS.md. Format:
 *  `- NO-SEED: <module> — <reason> [approved: <who/when>]` (reason + sign-off required). */
export function parseSeedExceptions(text) {
  const set = new Set()
  // Require a reason AND a CLOSED `[approved: … ]` token (an unterminated
  // `[approved:` is a malformed sign-off and must not count).
  const re = /^-\s*NO-SEED:\s*(\S+)\s+[—-]\s.*\[approved:[^\]]*\]/gim
  let m
  while ((m = re.exec(text || '')) !== null) set.add(m[1].trim())
  return set
}

/**
 * Pure set-difference (exported for TEST-1/2/15). `modules` is
 * `{ name, hasSurface, hasSeed }[]`; `allowlist` is a Set of module names.
 *   - missing: surface-bearing, un-seeded, un-allow-listed → FAIL.
 *   - stale:   allow-listed but now seeded OR no longer a surface → FAIL (GC).
 */
export function computeSeedDrift(modules, allowlist) {
  const byName = new Map(modules.map(m => [m.name, m]))
  const missing = modules
    .filter(m => m.hasSurface && !m.hasSeed && !allowlist.has(m.name))
    .map(m => m.name)
  const stale = [...allowlist].filter(name => {
    const m = byName.get(name)
    return !m || m.hasSeed || !m.hasSurface
  })
  return { missing, stale }
}

/** Enumerate every module dir → { name, hasSurface, hasSeed }. */
export function enumerateModules(modulesDir = MODULES_DIR) {
  if (!fs.existsSync(modulesDir)) return []
  return fs
    .readdirSync(modulesDir)
    .filter(e => fs.statSync(path.join(modulesDir, e)).isDirectory())
    .map(name => {
      const dir = path.join(modulesDir, name)
      const moduleTsx = path.join(dir, 'module.tsx')
      const src = fs.existsSync(moduleTsx) ? fs.readFileSync(moduleTsx, 'utf-8') : ''
      return { name, hasSurface: hasUserSurface(src), hasSeed: hasSeed(dir) }
    })
    .sort((a, b) => a.name.localeCompare(b.name))
}

// Portable main-module check (the naive `file://${argv[1]}` is false on Windows
// — `file:///C:/…` vs `C:\…` — and on paths with spaces, which would silently
// disable the gate).
const isMain = import.meta.url === pathToFileURL(process.argv[1]).href
if (isMain) {
  const modules = enumerateModules()
  const exceptionsText = fs.existsSync(EXCEPTIONS_PATH)
    ? fs.readFileSync(EXCEPTIONS_PATH, 'utf-8')
    : ''
  const allowlist = parseSeedExceptions(exceptionsText)
  const { missing, stale } = computeSeedDrift(modules, allowlist)

  const statusOf = m => {
    if (m.hasSeed) return '✓ gallery.tsx'
    if (allowlist.has(m.name)) return '— allow-listed (no-seed)'
    if (!m.hasSurface) return '— no user surface'
    return '❌ MISSING seed'
  }
  const rows = modules
    .map(
      m =>
        `| \`${m.name}\` | ${m.hasSurface ? 'yes' : 'no'} | ${statusOf(m)} |`,
    )
    .join('\n')

  const seeded = modules.filter(m => m.hasSeed).length
  const body = `<!-- AUTO-GENERATED by scripts/gen-gallery-seed-registry.mjs — DO NOT EDIT. -->
# Dev-Gallery Seed Manifest

The living index of per-module dev-gallery seed ownership. Regenerate with
\`node scripts/gen-gallery-seed-registry.mjs\`; \`--check\` runs in \`npm run check\`.

A module with a user-facing surface (a non-skip route \`path:\` or a user-facing
slot) MUST own a \`src/modules/<X>/gallery.tsx\` (\`export const gallery\`), or be
listed in \`GALLERY_SEED_EXCEPTIONS.md\` with a structural reason + sign-off.

${modules.length} module${modules.length === 1 ? '' : 's'} · ${seeded} with a gallery.tsx · ${allowlist.size} allow-listed.

| Module | User surface? | Seed status |
|---|---|---|
${rows || '| _(none)_ | | |'}
`

  const check = process.argv.includes('--check')
  let failed = false

  if (missing.length) {
    failed = true
    console.error(
      `gallery seed drift: ${missing.length} surface-bearing module(s) have NO src/modules/<X>/gallery.tsx:\n` +
        missing.map(m => `    ${m}`).join('\n') +
        `\n  → add src/modules/<module>/gallery.tsx (export const gallery: ModuleGallery — see src/dev/gallery/support), ` +
        `or record an approved "- NO-SEED: <module> — <reason> [approved: …]" in src/dev/gallery/GALLERY_SEED_EXCEPTIONS.md.`,
    )
  }
  if (stale.length) {
    failed = true
    console.error(
      `gallery seed drift: ${stale.length} stale GALLERY_SEED_EXCEPTIONS entry(ies) (module now seeded OR no longer a surface): ${stale.join(', ')} — remove the NO-SEED line.`,
    )
  }

  if (check) {
    const cur = fs.existsSync(OUT) ? fs.readFileSync(OUT, 'utf-8') : ''
    if (cur.trim() !== body.trim()) {
      failed = true
      console.error(
        'GALLERY_SEED_MANIFEST.md is stale — run `node scripts/gen-gallery-seed-registry.mjs` and commit.',
      )
    }
    if (failed) process.exit(1)
    console.log(
      `gallery seed registry OK — ${modules.length} module(s), ${seeded} seeded, ${allowlist.size} allow-listed.`,
    )
  } else {
    fs.writeFileSync(OUT, body)
    console.log(`Wrote ${OUT} — ${modules.length} module(s), ${seeded} seeded.`)
    if (failed) process.exit(1)
  }
}
