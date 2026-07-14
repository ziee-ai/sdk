/**
 * TEST-14 / ITEM-16 — prove the per-module `gallery.tsx` seed (and the whole dev
 * gallery) is NEVER in the production app bundle.
 *
 * The gallery is reachable only from the dev-gallery chunk: the standalone
 * `gallery.html` entry is not a prod `rollupOptions.input`, and the in-app
 * dev-gallery route is `import.meta.env.DEV`-gated. The runtime registry carries
 * the sentinel `ZIEE_GALLERY_SEED_MARKER`; a correct prod build tree-shakes the
 * gallery out entirely, so the sentinel must be ABSENT from every emitted asset.
 *
 * Run: node scripts/check-gallery-prod-exclusion.mjs         (uses existing dist/ui)
 *      node scripts/check-gallery-prod-exclusion.mjs --build (vite build first)
 */
import { execSync } from 'node:child_process'
import path from 'node:path'
import fs from 'node:fs'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

// Config-driven anchors (was HERE/UI hardcodes). The app runs this with its
// `ui/` as cwd, so `gallery.config.json` + relative paths resolve against it.
const CFG = resolveGalleryConfig()
const UI = CFG.__cwd
// vite `root: 'src'` + `outDir: '../../dist/ui'` → src-app/dist/ui (CFG.distDir).
const DIST = path.resolve(UI, CFG.distDir)
const MARKER = CFG.prodMarker

if (process.argv.includes('--build')) {
  // Clean first — vite does NOT auto-empty an outDir that lives outside its root
  // (`dist/ui` is outside `src/`), so stale chunks from a prior (e.g. dev) build
  // would give a false verdict either way.
  console.log('cleaning dist/ui + building prod bundle (vite build)…')
  fs.rmSync(DIST, { recursive: true, force: true })
  execSync(CFG.buildCmd, { cwd: UI, stdio: 'inherit' })
}

if (!fs.existsSync(DIST)) {
  console.error(
    `prod-exclusion: no build at ${DIST} — run with --build (or \`npm run build\`) first.`,
  )
  process.exit(1)
}

function walk(dir, acc = []) {
  for (const e of fs.readdirSync(dir)) {
    const full = path.join(dir, e)
    if (fs.statSync(full).isDirectory()) walk(full, acc)
    else if (/\.(js|mjs|cjs)$/.test(e)) acc.push(full)
  }
  return acc
}

const offenders = walk(DIST).filter(f =>
  fs.readFileSync(f, 'utf-8').includes(MARKER),
)

if (offenders.length) {
  console.error(
    `prod-exclusion FAIL: the dev-gallery sentinel "${MARKER}" leaked into the prod bundle:\n` +
      offenders.map(f => `    ${path.relative(DIST, f)}`).join('\n') +
      `\n  → the gallery must be dev-only. Gate the dev-gallery lazy import behind ` +
      `import.meta.env.DEV so the reference is dropped in prod.`,
  )
  process.exit(1)
}

console.log(
  `prod-exclusion OK — "${MARKER}" absent from all ${walk(DIST).length} JS assets in dist/ui (gallery excluded from prod).`,
)
