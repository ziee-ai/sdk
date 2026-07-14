/**
 * Resolve the app's `gallery.config.json` (from cwd) into the parameters the
 * generic gallery scripts anchor on. Every field defaults to ziee's historical
 * hardcode, so an app that ships no config file behaves exactly as before.
 *
 * The scripts are shipped under `@ziee/gallery/scripts/*` and run with the app's
 * `ui/` as cwd (`node node_modules/@ziee/gallery/scripts/<x>.mjs`), so relative
 * paths (galleryDir, surfaceRoots, runtimeBaselineModule) resolve against it.
 */
import fs from 'node:fs'
import path from 'node:path'

const DEFAULTS = {
  /** Output/anchor dir (also where RUNTIME_FINDINGS + generated artifacts land). */
  galleryDir: 'src/dev/gallery',
  /** Roots the registry generators (testid/state-matrix/overlay) walk. */
  surfaceRoots: ['src/modules', 'src/components/ui'],
  /** Kit import specifier (overlay-registry `isKit()`). */
  kitImport: '@ziee/kit',
  /** Dev-server port for the gate + runtime passes. */
  port: 1420,
  /** Standalone gallery URL. */
  galleryUrl: '/gallery.html',
  /** Prod-exclusion marker (must equal the marker `@ziee/gallery` emits). */
  prodMarker: 'ZIEE_GALLERY_SEED_MARKER',
  /** Prod bundle dir (prod-exclusion check). */
  distDir: '../../dist/ui',
  /** Prod build command (prod-exclusion check). */
  buildCmd: 'npm run build:nocheck',
  /** Dev-server command the gate boots when the port is free. */
  devCmd: ['npm', 'run', 'dev'],
  /** Additional source trees (desktop) the dual-tree generators scan. */
  extraTrees: [],
  /** App module exporting `isRuntimeBaselined(finding)` (or null = none). */
  runtimeBaselineModule: null,
  /** Playwright visual config + spec list the gate runs. */
  visualConfig: 'playwright.visual.config.ts',
  visualSpecs: ['layout.spec.ts', 'states.spec.ts', 'overlays.spec.ts'],
  visualSnapshotSpecs: ['gallery.spec.ts'],
  /** Lint commands the gate runs (npm-script names). */
  lintCmds: [['npm', ['run', 'lint:guardrails']], ['npm', ['run', 'lint:colors']]],
}

/** Load + merge `gallery.config.json` from `cwd` over the defaults. */
export function resolveGalleryConfig(cwd = process.cwd()) {
  const configPath = path.resolve(cwd, 'gallery.config.json')
  let file = {}
  if (fs.existsSync(configPath)) {
    try {
      file = JSON.parse(fs.readFileSync(configPath, 'utf8'))
    } catch (e) {
      throw new Error(`[gallery-config] failed to parse ${configPath}: ${e.message}`)
    }
  }
  return { ...DEFAULTS, ...file, __cwd: cwd, __configPath: configPath }
}
