/**
 * Base Playwright config presets, parameterized over `E2EConfig`.
 *
 *   - `definePlaywrightPreset(cfg)` — the FULL-STACK server suite. Each test spins
 *     its OWN backend + Vite + a per-test DB cloned from the run's Postgres
 *     container (brought up by the shared `createGlobalSetup`); the app's fixtures
 *     set `baseURL` per worker. Defaults: workers=1 (the validated-stable value;
 *     env `PLAYWRIGHT_WORKERS` raises it), 3-min per-test timeout, chromium.
 *
 *   - `defineDesktopPreset(cfg)` — the desktop variant. Boots the Tauri/headless
 *     dev server via `webServer` and points every test at its `baseURL`. Longer
 *     timeout (first test waits on a cold backend build).
 *
 * Both wire the app's `createGlobalSetup(cfg)` / `createGlobalTeardown(cfg)` via
 * the `globalSetup`/`globalTeardown` MODULE PATHS the app re-exports (Playwright
 * requires file paths there, not function references), passed in `opts`.
 */
import { defineConfig, devices, type PlaywrightTestConfig } from '@playwright/test'
import crypto from 'node:crypto'
import type { E2EConfig } from './config.ts'

/** Module paths Playwright loads for global setup/teardown (default-exporting the fn). */
export interface PresetPaths {
  globalSetup?: string
  globalTeardown?: string
}

function runIdDirs(): { outputDir: string; reportDir: string } {
  const testRunId = process.env.TEST_RUN_ID || crypto.randomBytes(4).toString('hex')
  if (!process.env.TEST_RUN_ID) process.env.TEST_RUN_ID = testRunId
  return {
    outputDir: `test-results/${testRunId}`,
    reportDir: `playwright-report/${testRunId}`,
  }
}

/** The FULL-STACK server E2E suite (per-test backend + Vite + Postgres). */
export function definePlaywrightPreset(
  cfg: E2EConfig,
  opts: PresetPaths = {},
): PlaywrightTestConfig {
  const { outputDir, reportDir } = runIdDirs()
  return defineConfig({
    testDir: cfg.testDir,
    testIgnore: cfg.testIgnore,
    outputDir,
    fullyParallel: true,
    forbidOnly: !!process.env.CI,
    retries: process.env.CI ? 2 : 0,
    workers: process.env.PLAYWRIGHT_WORKERS
      ? Number(process.env.PLAYWRIGHT_WORKERS)
      : (cfg.workers ?? 1),
    reporter: [
      ['html', { outputFolder: reportDir, open: 'never' }],
      ['junit', { outputFile: `${reportDir}/results.xml` }],
      ['list'],
    ],
    use: {
      trace: 'retain-on-failure',
      screenshot: 'only-on-failure',
      video: 'retain-on-failure',
      actionTimeout: 10_000,
    },
    globalSetup: opts.globalSetup,
    globalTeardown: opts.globalTeardown,
    projects: [
      {
        name: 'chromium',
        use: { ...devices['Desktop Chrome'], viewport: cfg.viewport ?? { width: 1280, height: 720 } },
      },
    ],
    timeout: cfg.timeout ?? 180_000,
    expect: { timeout: 10_000 },
  })
}

/** The DESKTOP variant — boots the Tauri/headless dev server via `webServer`. */
export function defineDesktopPreset(
  cfg: E2EConfig,
  opts: PresetPaths = {},
): PlaywrightTestConfig {
  const { outputDir, reportDir } = runIdDirs()
  const baseURL = cfg.baseURL ?? 'http://localhost:1420'
  return defineConfig({
    testDir: cfg.testDir,
    testIgnore: cfg.testIgnore,
    outputDir,
    fullyParallel: true,
    forbidOnly: !!process.env.CI,
    retries: process.env.CI ? 2 : 0,
    workers: cfg.workers ?? 4,
    reporter: [
      ['html', { outputFolder: reportDir, open: 'never' }],
      ['junit', { outputFile: `${reportDir}/results.xml` }],
      ['list'],
    ],
    use: {
      baseURL,
      trace: 'retain-on-failure',
      screenshot: 'only-on-failure',
      video: 'retain-on-failure',
      actionTimeout: 10_000,
    },
    globalSetup: opts.globalSetup,
    globalTeardown: opts.globalTeardown,
    projects: [
      {
        name: 'chromium',
        use: { ...devices['Desktop Chrome'], viewport: cfg.viewport ?? { width: 1280, height: 720 } },
      },
    ],
    timeout: cfg.timeout ?? 300_000,
    expect: { timeout: 10_000 },
    webServer: cfg.webServer
      ? {
          command: cfg.webServer.command,
          url: cfg.webServer.url,
          reuseExistingServer: cfg.webServer.reuseExistingServer ?? !process.env.CI,
          timeout: cfg.webServer.timeout ?? 120_000,
        }
      : undefined,
  })
}
