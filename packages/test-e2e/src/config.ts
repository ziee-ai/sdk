/**
 * `@ziee/test-e2e` — the ONE config seam every scaffold piece anchors on.
 *
 * A ziee-SDK app supplies its OWN specs + fixtures; this package supplies the
 * reusable Layer-B scaffold (Playwright preset + Postgres-docker global setup /
 * teardown + port allocator + testid selectors), all parameterized over
 * `E2EConfig`. Nothing here hardcodes a ziee path — the app passes its server
 * command, base URL, test dir, and docker template.
 *
 * Two presets share the config:
 *   - `definePlaywrightPreset(cfg)` — the FULL-STACK server suite (per-test
 *     backend + Vite + a per-run Postgres container from `globalSetup`).
 *   - `defineDesktopPreset(cfg)` — the desktop variant (boots the Tauri/headless
 *     dev server via `webServer`; no per-test Postgres).
 */

/** How the global-setup builds + serves the UI once per run (static preview). */
export interface UiBuildConfig {
  /** Vite `root` (usually `<uiRoot>/src`). */
  root: string
  /** Build output dir (served by the app's `webServer`/preview). */
  outDir: string
  /** Vite cache dir (defaults under `<uiRoot>/node_modules`). */
  cacheDir?: string
  /**
   * Singletons to force a single copy of into the bundle (react/immer/zustand/…).
   * A second copy of a stateful singleton boot-crashes the app; keep this in
   * lockstep with the app's `vite.config` dedupe list.
   */
  dedupe?: string[]
  /** Extra deps to prebundle (`optimizeDeps.include`). */
  optimizeDepsInclude?: string[]
  /** Skip the build + reuse an existing `outDir` when this env var === '1'. */
  skipEnv?: string
}

/** How the global-setup warms the backend binary once (out of the per-test budget). */
export interface ServerWarmupConfig {
  /** Working dir the build command runs in (the server crate root). */
  cwd: string
  /** Build command + args (e.g. `['cargo', ['build', '--bin', 'ziee']]`). */
  command: [string, string[]]
  /** Skip warmup when this env var === '1'. */
  skipEnv?: string
}

export interface E2EConfig {
  // ── Playwright preset ─────────────────────────────────────────────────────
  /** Test dir (relative to the config file). */
  testDir: string
  /** Glob(s) of specs to ignore (e.g. the backend-free visual specs). */
  testIgnore?: string | RegExp | (string | RegExp)[]
  /** Base URL — set for the desktop preset (the full-stack preset sets it per worker). */
  baseURL?: string
  /** Per-test timeout ms (default 180_000; desktop 300_000). */
  timeout?: number
  /** Worker count (default 1 for the full-stack suite; env `PLAYWRIGHT_WORKERS` wins). */
  workers?: number
  /** Chromium viewport (default 1280×720). */
  viewport?: { width: number; height: number }
  /** A `webServer` block — the desktop preset needs one (Tauri/headless dev server). */
  webServer?: {
    command: string
    url: string
    timeout?: number
    reuseExistingServer?: boolean
  }

  // ── global-setup / teardown ───────────────────────────────────────────────
  /**
   * Path to the docker-compose TEMPLATE with `${RUN_ID}` + `${POSTGRES_PORT}`
   * placeholders (pgvector image, healthcheck, tuning). Omit to skip the
   * Postgres container entirely (a desktop/headless suite with its own DB).
   */
  dockerComposeTemplate?: string
  /** Container name prefix — MUST match the template's `container_name` prefix. */
  containerNamePrefix?: string
  /** Compose service/project namespace seed (default derived from the base PG port). */
  basePgPortEnv?: string
  /** Default base Postgres port when `basePgPortEnv` is unset (default 54331). */
  defaultBasePgPort?: number
  /** Dir the setup writes per-run configs + lock bookkeeping into (relative to config file). */
  configDir?: string
  /** `.env` file to load before setup (relative to config file). */
  envFile?: string
  /** Build + serve the UI once per run (static preview). Omit to skip. */
  uiBuild?: UiBuildConfig
  /** Warm the server binary once per run. Omit to skip. */
  serverWarmup?: ServerWarmupConfig
  /**
   * Optional first-run setup hook, run once after the container is ready — e.g.
   * to seed the admin account against a booted server. Most apps do first-run
   * setup PER-TEST in a fixture instead; this is the global escape hatch.
   */
  firstRunSetup?: (ctx: { runId: string; postgresPort: number }) => Promise<void>
}

/** Identity helper — gives editors the `E2EConfig` type on a plain object literal. */
export function defineE2EConfig(cfg: E2EConfig): E2EConfig {
  return cfg
}
