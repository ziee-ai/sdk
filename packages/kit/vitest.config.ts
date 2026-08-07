import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

// Unit runner for @ziee/kit. Most of the kit is exercised through the consuming app's suites; this
// pass exists for behaviour that can ONLY be asserted about the kit itself — currently the
// cross-window portal contract (kit/portal-container.test.tsx), which needs two real documents and
// so cannot be expressed in a consumer's single-document render test.

const here = fileURLToPath(new URL('.', import.meta.url))

/**
 * `node:test` suites are excluded, DERIVED rather than hand-listed — vitest reports a `node:test`
 * file as a passing file with zero tests, so leaving one in `include` turns a real suite into a
 * hollow green tick (the same trap the app's vitest.config.ts documents). Run those with
 * `node --test`.
 */
function nodeTestFiles(): string[] {
  const dir = join(here, 'src', 'kit')
  return readdirSync(dir)
    .filter(f => /\.test\.tsx?$/.test(f))
    .filter(f => /from ['"]node:test['"]/.test(readFileSync(join(dir, f), 'utf8')))
    .map(f => `src/kit/${f}`)
}

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    exclude: ['**/node_modules/**', ...nodeTestFiles()],
  },
})
