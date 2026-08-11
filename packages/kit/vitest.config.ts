import { defineConfig } from 'vitest/config'
import { fileURLToPath } from 'node:url'
// @ts-expect-error - plain .mjs tooling shared by every package's config + node runner
import { listNodeTestFiles } from '../../scripts/node-test-files.mjs'

// Unit runner for @ziee/kit. Most of the kit is exercised through the consuming app's suites; this
// pass exists for behaviour that can ONLY be asserted about the kit itself — currently the
// cross-window portal contract (kit/portal-container.test.tsx), which needs two real documents and
// so cannot be expressed in a consumer's single-document render test.

const here = fileURLToPath(new URL('.', import.meta.url))

/**
 * `node:test` suites are excluded, DERIVED rather than hand-listed — vitest reports a `node:test`
 * file as a passing file with zero tests, so leaving one in `include` turns a real suite into a
 * hollow green tick (the same trap the app's vitest.config.ts documents).
 *
 * The derivation now comes from the SHARED `scripts/node-test-files.mjs`, which is also what
 * `npm run test:node` uses to RUN them. Previously this config excluded them and nothing ran them,
 * so `src/kit/table-view-core.test.ts` (14 tests) was dark; one criterion, two consumers, no gap.
 */
export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    exclude: ['**/node_modules/**', ...listNodeTestFiles(here)],
  },
})
