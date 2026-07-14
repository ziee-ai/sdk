/**
 * `createGlobalSetup(cfg)` — the parameterized E2E global-setup.
 *
 * Returns a Playwright `globalSetup` function that, ONCE per run:
 *   1. ensures the chromium browser binary,
 *   2. reaps stale port-locks + THIS session's crashed Postgres containers
 *      (cross-session-safe: liveness from the shared lock dir, never a sibling's
 *      per-worktree file),
 *   3. allocates a Postgres port + renders the app's docker-compose template,
 *   4. brings the container up + waits until it accepts connections,
 *   5. (optional) builds the UI once for static preview,
 *   6. (optional) warms the server binary out of the per-test budget,
 *   7. (optional) runs the app's first-run setup hook.
 *
 * Everything app-specific — the compose template, container prefix, UI build
 * options, server-warmup command, first-run seeding — comes from `E2EConfig`.
 * The per-run `{runId, port, dockerComposePath}` is written to
 * `<configDir>/postgres-<runId>.json` for the teardown + the app's per-test
 * fixtures to read.
 */
import type { FullConfig } from '@playwright/test'
import { execSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import crypto from 'node:crypto'
import type { E2EConfig } from './config.ts'
import {
  allocatePostgresPort,
  cleanupStaleConfigFiles,
  cleanupStaleLocks,
  liveRunIds,
} from './port-manager.ts'

const sleep = (ms: number) => new Promise(r => setTimeout(r, ms))

export function createGlobalSetup(cfg: E2EConfig) {
  return async function globalSetup(pwConfig: FullConfig): Promise<void> {
    // Resolve app-relative paths against the config FILE's dir (Playwright passes
    // it), falling back to cwd.
    const rootDir =
      (pwConfig as { rootDir?: string }).rootDir ??
      dirname(fileURLToPath(pwConfig.configFile ?? `file://${process.cwd()}/`))
    const rel = (p: string) => resolve(rootDir, p)

    console.log('\n🚀 Starting @ziee/test-e2e infrastructure...\n')

    if (cfg.envFile && existsSync(rel(cfg.envFile))) {
      // dotenv is a peer/dev dep; import lazily so a suite that sets no envFile
      // needn't install it.
      const dotenv = await import('dotenv')
      dotenv.config({ path: rel(cfg.envFile) })
    }

    // 1. Ensure chromium (idempotent + fast when cached).
    try {
      console.log('🌐 Ensuring Playwright chromium is installed...')
      execSync('npx playwright install chromium', { stdio: 'inherit' })
    } catch (e) {
      console.warn('⚠️  playwright install chromium failed (continuing):', e)
    }

    const configDir = rel(cfg.configDir ?? 'tests/.test-configs')
    if (!existsSync(configDir)) mkdirSync(configDir, { recursive: true })
    cleanupStaleLocks()
    cleanupStaleConfigFiles(configDir)

    const basePort =
      Number(process.env[cfg.basePgPortEnv ?? 'ZIEE_E2E_BASE_PG_PORT']) ||
      (cfg.defaultBasePgPort ?? 54331)
    const sessionNs = `pg${basePort}`
    const runId = process.env.TEST_RUN_ID || `${sessionNs}-${crypto.randomBytes(4).toString('hex')}`
    process.env.TEST_RUN_ID = runId
    console.log(`🆔 Test run ID: ${runId}`)

    // 2 + 3 + 4 need a Postgres container. Skip the whole block for a suite that
    // manages its own DB (no dockerComposeTemplate).
    let postgresPort = 0
    let dockerComposePath = ''
    if (cfg.dockerComposeTemplate) {
      const prefix = cfg.containerNamePrefix ?? 'ziee-e2e-postgres'
      // Reap only THIS session's crashed containers; a live lock protects a
      // sibling session's running one.
      console.log('🧹 Cleaning up stale PostgreSQL containers...')
      try {
        const names = execSync(
          `docker ps -a --filter "name=${prefix}-${sessionNs}-" --format "{{.Names}}"`,
          { encoding: 'utf-8' },
        ).trim()
        if (names) {
          const live = liveRunIds()
          for (const name of names.split('\n')) {
            const rid = name.replace(`${prefix}-`, '')
            if (live.has(rid)) {
              console.log(`   ✅ Kept active container: ${name} (live lock)`)
            } else {
              console.log(`   🗑️  Removing stale container: ${name}`)
              execSync(`docker rm -f ${name}`, { stdio: 'ignore' })
            }
          }
        }
      } catch {
        /* no containers */
      }

      console.log('🔍 Allocating PostgreSQL port...')
      postgresPort = await allocatePostgresPort(runId, basePort)
      console.log(`✅ Allocated PostgreSQL port: ${postgresPort}\n`)

      const template = readFileSync(rel(cfg.dockerComposeTemplate), 'utf-8')
        .replace(/\$\{RUN_ID\}/g, runId)
        .replace(/\$\{POSTGRES_PORT\}/g, String(postgresPort))
      dockerComposePath = resolve(configDir, `docker-compose-${runId}.yaml`)
      writeFileSync(dockerComposePath, template)

      writeFileSync(
        resolve(configDir, `postgres-${runId}.json`),
        JSON.stringify({ runId, port: postgresPort, dockerComposePath }, null, 2),
      )

      console.log(`🐘 Starting PostgreSQL container for run ${runId}...`)
      execSync(`docker compose -f "${dockerComposePath}" up -d`, { stdio: 'inherit' })

      console.log('⏳ Waiting for PostgreSQL to be ready...')
      await sleep(3000)
      await waitForPostgres(postgresPort)
      console.log('✅ PostgreSQL ready for tests!\n')
    }

    // 5. Build the UI once for static preview (opt-out via cfg.uiBuild.skipEnv).
    if (cfg.uiBuild) await buildUiOnce(cfg.uiBuild, configDir)

    // 6. Warm the server binary once (opt-out via cfg.serverWarmup.skipEnv).
    if (cfg.serverWarmup) warmServer(cfg.serverWarmup)

    // 7. App first-run setup hook.
    if (cfg.firstRunSetup) {
      console.log('🔑 Running app first-run setup hook...')
      await cfg.firstRunSetup({ runId, postgresPort })
    }

    console.log('   Test infrastructure ready.\n')
  }
}

async function waitForPostgres(port: number): Promise<void> {
  // pg is a peer/dev dep; import lazily so a no-DB suite needn't install it.
  const pg = (await import('pg')).default
  const pool = new pg.Pool({
    host: 'localhost',
    port,
    user: 'postgres',
    password: 'password',
    database: 'postgres',
  })
  let retries = 30
  while (retries > 0) {
    try {
      await pool.query('SELECT 1')
      console.log('✅ Connected to test PostgreSQL\n')
      break
    } catch (error) {
      if (--retries === 0) {
        await pool.end()
        throw error
      }
      await sleep(1000)
    }
  }
  await pool.end()
}

function buildUiOnce(b: NonNullable<E2EConfig['uiBuild']>, configDir: string): void {
  if (b.skipEnv && process.env[b.skipEnv] === '1' && existsSync(resolve(b.outDir, 'index.html'))) {
    console.log(`🏗️  ${b.skipEnv}=1 — reusing existing build\n`)
    return
  }
  console.log('🏗️  Building UI for static preview (once per run)...')
  const buildCfg = resolve(configDir, 'vite-e2e-build.ts')
  writeFileSync(
    buildCfg,
    `import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  root: ${JSON.stringify(b.root)},
  cacheDir: ${JSON.stringify(b.cacheDir ?? resolve(b.root, '../node_modules/.vite-e2e-build'))},
  resolve: { dedupe: ${JSON.stringify(b.dedupe ?? [])} },
  optimizeDeps: { include: ${JSON.stringify(b.optimizeDepsInclude ?? [])} },
  build: { outDir: ${JSON.stringify(b.outDir)}, emptyOutDir: true },
})
`,
  )
  execSync(`npx vite build --config "${buildCfg}"`, { cwd: dirname(b.root), stdio: 'inherit' })
  console.log('✅ UI build ready for preview\n')
}

function warmServer(w: NonNullable<E2EConfig['serverWarmup']>): void {
  if (w.skipEnv && process.env[w.skipEnv] === '1') return
  const [cmd, args] = w.command
  console.log(`🏗️  Warming the server binary (${cmd} ${args.join(' ')})...`)
  try {
    execSync(`${cmd} ${args.join(' ')}`, { cwd: w.cwd, stdio: 'inherit' })
    console.log('✅ Server binary warm\n')
  } catch (e) {
    console.warn('⚠️  Server warmup failed (continuing; per-test build will pay it):', e)
  }
}
