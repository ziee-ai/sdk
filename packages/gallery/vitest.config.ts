import { defineConfig } from 'vitest/config'
import { fileURLToPath } from 'node:url'
// @ts-expect-error - plain .mjs tooling shared by every package's config + node runner
import { listNodeTestFiles } from '../../scripts/node-test-files.mjs'

// Unit runner for @ziee/gallery. Mirrors @ziee/kit's config; see
// `scripts/node-test-files.mjs` for why the `node:test` half is excluded HERE and run by
// `npm run test:node`.
//
// EVERY one of this package's 7 suites is currently `node:test` (3 under `src/`, 4 under
// `scripts/`), so after the exclusion vitest collects nothing — hence `passWithNoTests`.
// The vitest pass is kept anyway, and deliberately: without it, the first `*.test.tsx`
// added here would have no runner at all, which is the exact defect this file was written
// to close. The `node:test` side is NOT protected by `passWithNoTests` — it fails closed
// against a committed floor in `test:node`.

const here = fileURLToPath(new URL('.', import.meta.url))

export default defineConfig({
  test: {
    environment: 'node',
    include: ['src/**/*.test.ts', 'src/**/*.test.tsx'],
    exclude: ['**/node_modules/**', ...listNodeTestFiles(here)],
    passWithNoTests: true,
  },
})
