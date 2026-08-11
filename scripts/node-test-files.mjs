/**
 * The single source of truth for "which test files belong to the `node:test` runner"
 * — shared by every package's `vitest.config.ts` (which must EXCLUDE them) and by
 * `run-node-tests.mjs` (which must RUN them).
 *
 * ## Why this exists
 *
 * vitest does not understand `node:test`. It COLLECTS such a file, sees no
 * `describe`/`it`, and reports it as a PASSING file with zero tests — even when the
 * file's assertions throw. So a `node:test` suite left inside vitest's `include` is
 * strictly worse than an unrun one: it is an unrun one that reports green.
 *
 * That trap was live in this repo. `@ziee/kit`'s `vitest.config.ts` already derived an
 * exclusion for exactly this reason, but nothing then RAN the excluded file, so
 * `kit/src/kit/table-view-core.test.ts` (14 tests) was dark. `@ziee/framework` (8 test
 * files) and `@ziee/gallery` (7) had no `test` script at all — 15 files, ~66 tests,
 * committed and never executed anywhere.
 *
 * Both consumers of this module derive from the SAME criterion, so a new `node:test`
 * suite is automatically excluded from vitest and automatically run by the node runner.
 * A file can never be silently claimed by both or by neither — which is the only
 * failure mode that matters here.
 *
 * ## The criterion
 *
 * A test file that imports the `node:test` module. That is not a heuristic — it is the
 * literal definition of "a file the node test runner executes and vitest cannot".
 */

import { readdirSync, readFileSync, existsSync } from 'node:fs'
import path from 'node:path'

/** `import … from 'node:test'` / `require('node:test')`, quote-agnostic. */
const NODE_TEST_IMPORT = /(?:from|require\s*\()\s*['"]node:test['"]/

/** Trees that must never be walked for test files. */
const SKIP_WALK = new Set(['node_modules', 'dist', 'build', 'coverage', '.vite', '.turbo', '.git'])

const TEST_FILE = /\.(test|spec)\.(ts|tsx|mts|cts|mjs|cjs|js|jsx)$/

/** Directories, relative to a package root, that hold its suites. */
export const DEFAULT_ROOTS = ['src', 'scripts']

function walk(dir, out = []) {
  if (!existsSync(dir)) return out
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    if (SKIP_WALK.has(entry.name) || entry.name.startsWith('.')) continue
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) {
      walk(full, out)
      continue
    }
    if (TEST_FILE.test(entry.name)) out.push(full)
  }
  return out
}

/**
 * Every test file under `<pkgRoot>/<root>` (for each `root`) that imports `node:test`,
 * as POSIX paths RELATIVE to `pkgRoot` — the form both vitest's `exclude` globs and
 * `node --test`'s arguments want. Sorted, for determinism.
 *
 * @param {string} pkgRoot absolute path to the package directory
 * @param {string[]} roots subdirectories of `pkgRoot` to scan
 * @returns {string[]}
 */
export function listNodeTestFiles(pkgRoot, roots = DEFAULT_ROOTS) {
  const files = roots.flatMap(r => walk(path.join(pkgRoot, r)))
  return files
    .filter(f => NODE_TEST_IMPORT.test(readFileSync(f, 'utf8')))
    .map(f => path.relative(pkgRoot, f).split(path.sep).join('/'))
    .sort()
}
