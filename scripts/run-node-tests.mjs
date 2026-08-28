#!/usr/bin/env node
/**
 * The `node:test` half of a package's `npm test`, shared by every SDK package.
 *
 * A package's suites are split across two runners that cannot see each other: vitest
 * (`*.test.tsx` component/DOM tests) and `node:test` (pure-logic tests). The file list
 * here is DERIVED from `node-test-files.mjs` — the same helper each package's
 * `vitest.config.ts` uses to EXCLUDE those files — rather than hand-maintained, so the
 * includer and the excluder can never disagree about who owns a file.
 *
 * Run from the package directory (npm sets cwd to it):
 *   node ../../scripts/run-node-tests.mjs --min <floor> [--roots src,scripts] [node --test args…]
 *
 *   --min N      FAIL CLOSED below N discovered files (see below). Required.
 *   --roots a,b  subdirectories to scan (default: src,scripts).
 */

import { spawnSync } from 'node:child_process'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { listNodeTestFiles, DEFAULT_ROOTS } from './node-test-files.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const PKG_ROOT = process.cwd()

const argv = process.argv.slice(2)
const takeOpt = name => {
  const i = argv.indexOf(name)
  if (i === -1) return null
  const [, value] = argv.splice(i, 2)
  return value ?? null
}

const minRaw = takeOpt('--min')
const rootsRaw = takeOpt('--roots')

if (minRaw === null || !/^\d+$/.test(minRaw)) {
  console.error(
    '[run-node-tests] --min <N> is REQUIRED. It is the committed floor on how many node:test ' +
      'files this package must discover; without it a rename that emptied the scan would exit 0 ' +
      'having run nothing, which is the exact failure this runner exists to prevent.',
  )
  process.exit(1)
}
const MIN = Number(minRaw)
const roots = rootsRaw ? rootsRaw.split(',').map(s => s.trim()).filter(Boolean) : DEFAULT_ROOTS

const files = listNodeTestFiles(PKG_ROOT, roots)

// FAIL CLOSED. `node --test` with an empty file list exits 0 having run nothing; a
// rename that moved the suites out of the scan would look exactly like a green run.
if (files.length < MIN) {
  console.error(
    `[run-node-tests] SCAN SET COLLAPSED — found ${files.length} node:test file(s) under ` +
      `${roots.map(r => `${r}/`).join(', ')} in ${PKG_ROOT}, below the committed floor of ${MIN}. ` +
      'A suite was moved or renamed out of the scan, so this runner was about to exit 0 having ' +
      'run almost nothing. Fix the move, or lower --min in package.json as a reviewable edit.',
  )
  process.exit(1)
}

console.log(`[run-node-tests] ${path.basename(PKG_ROOT)}: running ${files.length} node:test file(s)`)

const result = spawnSync(
  process.execPath,
  [
    '--test',
    '--experimental-strip-types',
    '--no-warnings=ExperimentalWarning',
    '--import',
    path.join(HERE, 'ts-resolve.mjs'),
    ...argv,
    ...files,
  ],
  { cwd: PKG_ROOT, stdio: 'inherit' },
)

process.exit(result.status ?? 1)
