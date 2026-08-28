import { defineConfig } from 'vitest/config'
import { fileURLToPath } from 'node:url'
// @ts-expect-error - plain .mjs tooling shared by every package's config + node runner
import { listNodeTestFiles } from '../../scripts/node-test-files.mjs'

// Unit runner for @ziee/framework. Mirrors @ziee/kit's config exactly; see
// `scripts/node-test-files.mjs` for why the `node:test` half is excluded HERE and run
// by `npm run test:node`.
//
// Most of this package's suites are `node:test` (7 of 8 files) — they assert pure store /
// seam / router-config logic and need no DOM. `passWithNoTests` is NOT set: vitest must
// keep collecting the DOM-shaped files (today `src/notification/registry.test.ts`), and an
// empty collection here would mean a real regression in the include glob.

const here = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    exclude: ['**/node_modules/**', ...listNodeTestFiles(here)],
  },
})
