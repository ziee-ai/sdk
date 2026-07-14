/**
 * `createGlobalTeardown(cfg)` — the parameterized E2E global-teardown.
 *
 * Reads the per-run `<configDir>/postgres-<runId>.json` the setup wrote, stops +
 * removes the run's Postgres container (with its volume), releases the port lock,
 * and deletes the per-run config files. A no-DB suite (no `dockerComposeTemplate`)
 * writes no config file, so teardown is a no-op.
 */
import type { FullConfig } from '@playwright/test'
import { execSync } from 'node:child_process'
import { existsSync, readFileSync, rmSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import type { E2EConfig } from './config.ts'
import { releasePostgresPortLock } from './port-manager.ts'

export function createGlobalTeardown(cfg: E2EConfig) {
  return async function globalTeardown(pwConfig: FullConfig): Promise<void> {
    console.log('\n🧹 Cleaning up @ziee/test-e2e infrastructure...\n')
    const runId = process.env.TEST_RUN_ID
    if (!runId) {
      console.log('⚠️  No TEST_RUN_ID — skipping cleanup')
      return
    }
    const rootDir =
      (pwConfig as { rootDir?: string }).rootDir ??
      dirname(fileURLToPath(pwConfig.configFile ?? `file://${process.cwd()}/`))
    const configDir = resolve(rootDir, cfg.configDir ?? 'tests/.test-configs')
    const configPath = resolve(configDir, `postgres-${runId}.json`)
    if (!existsSync(configPath)) {
      console.log('⚠️  No per-run config file — nothing to tear down')
      return
    }
    try {
      const { port, dockerComposePath } = JSON.parse(readFileSync(configPath, 'utf-8'))
      console.log(`🛑 Stopping PostgreSQL container for run ${runId}...`)
      try {
        execSync(`docker compose -f "${dockerComposePath}" down -v`, { stdio: 'inherit' })
      } catch (e) {
        console.error('❌ Failed to stop PostgreSQL container:', e)
      }
      releasePostgresPortLock(port)
      rmSync(dockerComposePath, { force: true })
      rmSync(configPath, { force: true })
      console.log('✅ Cleanup complete!\n')
    } catch (e) {
      console.error('❌ Error during cleanup:', e)
    }
  }
}
