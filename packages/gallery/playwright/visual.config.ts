/**
 * `defineVisualConfig(overrides?)` — the config-driven Visual-testing Playwright
 * preset (Layers A + B over the component gallery), re-homed under `@ziee/gallery`.
 *
 * Distinct from the full E2E suite in one decisive way: the gallery is a dev-only,
 * BACKEND-FREE canvas, so this boots ONLY the gallery Vite server (serving
 * `galleryUrl`) — no Postgres, no server, no global setup — which makes the visual
 * layers fast + deterministic.
 *
 * Every knob comes from the app's `gallery.config.json` (the SAME file the runner
 * gates + generators read), so a fresh app's `playwright.visual.config.ts` is just:
 *
 *   import { defineVisualConfig } from '@ziee/gallery/playwright/visual.config'
 *   export default defineVisualConfig()
 *
 * Run:
 *   npx playwright test -c playwright.visual.config.ts            # A + B
 *   npx playwright test -c playwright.visual.config.ts --list     # resolve only
 *   npx playwright test -c playwright.visual.config.ts --update-snapshots  # bless B
 */
import { defineConfig, devices, type PlaywrightTestConfig } from '@playwright/test'
import { existsSync, readFileSync } from 'node:fs'
import { resolve } from 'node:path'

interface VisualConfigFile {
  /** Gallery Vite dev-server port (default 1420). */
  port?: number
  /** Standalone gallery URL (default `/gallery.html`). */
  galleryUrl?: string
  /** Visual spec dir, relative to the app root (default `./tests/e2e/visual`). */
  visualTestDir?: string
  /** Dev-server command (default `['npm', 'run', 'dev']`). */
  devCmd?: string[]
  /** toHaveScreenshot maxDiffPixelRatio (default 0.05 — font-AA headroom). */
  maxDiffPixelRatio?: number
}

/** Load the visual-relevant fields from the app's `gallery.config.json` (cwd). */
function loadConfig(): Required<VisualConfigFile> {
  const defaults: Required<VisualConfigFile> = {
    port: 1420,
    galleryUrl: '/gallery.html',
    visualTestDir: './tests/e2e/visual',
    devCmd: ['npm', 'run', 'dev'],
    maxDiffPixelRatio: 0.05,
  }
  const p = resolve(process.cwd(), 'gallery.config.json')
  if (!existsSync(p)) return defaults
  try {
    const file = JSON.parse(readFileSync(p, 'utf8')) as VisualConfigFile
    return { ...defaults, ...file }
  } catch {
    return defaults
  }
}

export function defineVisualConfig(overrides: Partial<VisualConfigFile> = {}): PlaywrightTestConfig {
  const cfg = { ...loadConfig(), ...overrides }
  const PORT = Number(process.env.GALLERY_PORT || cfg.port)
  const BASE_URL = `http://localhost:${PORT}`
  const [devCmd, ...devArgs] = cfg.devCmd

  return defineConfig({
    testDir: cfg.visualTestDir,
    fullyParallel: true,
    forbidOnly: !!process.env.CI,
    retries: process.env.CI ? 1 : 0,
    workers: process.env.PLAYWRIGHT_WORKERS ? Number(process.env.PLAYWRIGHT_WORKERS) : undefined,
    reporter: [['list'], ['html', { open: 'never' }]],

    // Boot the gallery Vite server. No backend — the standalone entry renders the
    // gallery under the real ThemeProvider against the mock-API cassette. Pass the
    // port to Vite (strictPort) so a GALLERY_PORT override actually takes.
    webServer: {
      command: `${[devCmd, ...devArgs].join(' ')} -- --port ${PORT} --strictPort`,
      url: `${BASE_URL}${cfg.galleryUrl}`,
      reuseExistingServer: !process.env.CI,
      timeout: 120_000,
      stdout: 'ignore',
      stderr: 'pipe',
    },

    use: { baseURL: BASE_URL, trace: 'retain-on-failure' },

    // Screenshot determinism: disable animations + caret; soak sub-pixel font AA
    // so unrelated machines don't false-fail. Playwright ANDs the thresholds, so
    // no maxDiffPixels floor (it would only make the gate stricter). True
    // cross-machine stability still requires blessing baselines in a pinned box.
    expect: {
      timeout: 10_000,
      toHaveScreenshot: {
        animations: 'disabled',
        caret: 'hide',
        maxDiffPixelRatio: cfg.maxDiffPixelRatio,
        scale: 'css',
      },
    },

    projects: [{ name: 'gallery', use: { ...devices['Desktop Chrome'] } }],
    timeout: 60_000,
  })
}

export default defineVisualConfig
