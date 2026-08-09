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
  /** The workspace `src/` root the registry generators anchor on (relative to cwd). */
  srcDir: 'src',
  /** Roots the registry generators (testid/state-matrix/overlay) walk. */
  surfaceRoots: ['src/modules', 'src/components/ui'],
  /** Kit import specifier (overlay-registry `isKit()`). */
  kitImport: '@ziee/kit',
  /** Import specifiers an overlay primitive is legitimately imported from
   *  (gen-overlay-registry). A binding from elsewhere sharing a primitive name
   *  is NOT counted. Each entry P matches `source === P || source.startsWith(P + '/')`. */
  overlayKitImports: [
    '@/components/ui',
    '@/modules/layouts/app-layout/components/Drawer',
  ],
  /** tsconfig (relative to cwd) the ts-morph state-matrix pass loads. */
  tsconfig: 'tsconfig.json',
  /**
   * Additional PACKAGE src roots (relative to cwd) the testid-registry generator
   * walks for static `data-testid` literals BEYOND the app trees (`srcDir` +
   * `extraTrees`). The kit/shell components moved into packages, so their own
   * testids are the source of truth for kit-component ids; the app walk adds the
   * app's ids and the two UNION into `testidOut`. `[]` = a self-contained app
   * whose kit ids still live under `srcDir` (the pre-package layout). */
  kitTestIds: [],
  /**
   * Output path (relative to cwd) for the generated testid registry. When the kit
   * is a package this is its `testIds.generated.ts` (the shared import surface
   * `@ziee/kit/testIds.generated`); `null` = the historical in-app default
   * `<srcDir>/components/ui/testIds.generated.ts`. */
  testidOut: null,
  /** Dev-server port for the gate + runtime passes. `null` = derive a
   *  per-worktree, bind-checked port from the unified run key (audit §7 — no
   *  fixed 1420). An app may still pin an explicit number, and GALLERY_PORT
   *  always overrides. */
  port: null,
  /** Which PORT_FLOORS range the key-derived port searches from (run-key.mjs).
   *  Web uses `webGallery`; the desktop workspace sets `desktopGallery` so the
   *  two never collide. */
  portWhich: 'webGallery',
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
  /**
   * The app's LIVE harness copies, for `check-harness-parity`. Either a STRING
   * path (relative to cwd) to a `{ "copies": [...] }` manifest — whose `file`
   * paths are relative to the MANIFEST, so several workspaces share ONE
   * declaration — or an inline array with `file` relative to cwd. `null` = no
   * consumer copies to check (a standalone package), and the guard says so and
   * exits 0 rather than hardcoding some app's layout. */
  harnessCopies: null,
  /** App module exporting `isRuntimeBaselined(finding)` (or null = none). */
  runtimeBaselineModule: null,
  /** Playwright visual config + spec list the gate runs. `null` = this app has
   *  no visual layer (no playwright.visual.config.ts), and the gate reports the
   *  step as "not configured" rather than failing on a config that isn't there.
   *  The desktop workspace is exactly that case. */
  visualConfig: 'playwright.visual.config.ts',
  visualSpecs: ['layout.spec.ts', 'states.spec.ts', 'overlays.spec.ts'],
  visualSnapshotSpecs: ['gallery.spec.ts'],
  /** EXTRA gate steps, as `[label, cmd, args]`. Same shape as `lintCmds` — a
   *  config-driven command list, so an app needing a stage the generic gate does
   *  not have declares it here instead of forking the whole script. The desktop
   *  workspace uses this for its gallery-coverage check. */
  gateExtraCmds: [],
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
  // An UNKNOWN key is a typo, and a typo here silently deletes behaviour: naming
  // the extra-steps key `gateExtraCommands` made the gate skip its coverage stage
  // entirely, print no line for it, and still exit 0. Config that quietly does
  // less than it says is the exact failure this whole area was fixed for.
  const unknown = Object.keys(file).filter(
    k => !(k in DEFAULTS) && !k.startsWith('$') && !k.startsWith('__'),
  )
  if (unknown.length)
    throw new Error(
      `[gallery-config] ${configPath} has unknown key(s): ${unknown.join(', ')}. ` +
        `Known keys: ${Object.keys(DEFAULTS).sort().join(', ')}. ` +
        `(A typo'd key is silently ignored, which removes the behaviour you meant ` +
        `to configure — so it is refused rather than defaulted.)`,
    )
  return { ...DEFAULTS, ...file, __cwd: cwd, __configPath: configPath }
}
