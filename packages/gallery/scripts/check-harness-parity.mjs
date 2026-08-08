/**
 * HARNESS PARITY GUARD — the gallery crawl harness exists in more than one tree,
 * and those copies have DRIFTED. This refuses a tree where a behavioural core is
 * present in one live copy and missing from another.
 *
 * ## Why
 *
 * At the time this was written the harness lived in three places:
 *
 *   | script              | sdk/packages/gallery | src-app/ui | src-app/desktop/ui |
 *   |---------------------|----------------------|------------|--------------------|
 *   | runtime-health.mjs  | LIVE (ui workspace)  | DEAD       | LIVE (desktop)     |
 *   | gate-ui.mjs         | LIVE (ui workspace)  | absent     | LIVE (desktop)     |
 *
 * The three had genuinely diverged (the desktop copy already muted a
 * `net::ERR_ABORTED` that the sdk copy did not; the sdk copy read `CFG.galleryDir`
 * where the desktop copy hardcoded `src/dev/gallery`). A fix applied to one copy
 * silently did not reach the others — which is the mechanism that let a defect
 * class survive several rounds of fixing.
 *
 * ## What it checks, and what it deliberately does NOT
 *
 * It asserts each live copy IMPORTS the shared behavioural module and CALLS its
 * entry point. It does not attempt to compare the copies' logic — a source-text
 * comparison of two files that legitimately differ is a guard with an unbounded
 * evasion space, and this repo has paid for that pattern twice. The real defence
 * is that the behaviour lives in ONE module (`lib/host-lock.mjs`,
 * `lib/run-validity.mjs`) which both copies import and which has its own unit
 * tests; this guard only proves the wiring is present in every copy.
 *
 * Exit 0 = every live copy carries every core. Exit 1 = a copy is missing one.
 *
 * Run: node check-harness-parity.mjs [--root <repo-root>]
 */
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const HERE = path.dirname(fileURLToPath(import.meta.url))

/**
 * Each core: the shared module that owns the behaviour, plus a call site that
 * proves the copy actually USES it (an import alone can be dead).
 */
export const CORES = [
  {
    id: 'host-lock',
    why: 'serializes crawls across worktrees on one host (D3)',
    module: 'host-lock.mjs',
    // Either entry point: the crawl wraps itself in withHostLock; the gate takes
    // the lock around only its shared-resource phase via acquire().
    callSite: /withHostLock\s*\(|acquire\s*\(\s*\{/,
  },
  {
    id: 'run-manifest',
    why: 'makes a killed crawl unable to inherit a previous run (D4)',
    module: 'run-validity.mjs',
    // A PRODUCER writes the manifest; a CONSUMER verifies it. One alternation
    // covering both would let a producer-only copy satisfy the consumer half and
    // vice-versa, so each side names the call IT must make.
    callSite: /writeRunManifest\s*\(/,
    consumerCallSite: /verifyRunManifest\s*\(/,
  },
  {
    id: 'run-validity',
    why: 'VOIDs a run whose origin died or whose findings are mostly transport noise (D1)',
    module: 'run-validity.mjs',
    // Specifically assessRun — NOT an alternation with clearRunArtifacts. The
    // looser form let a copy delete the entire void-assessment and stay green on
    // the unrelated clear call, i.e. the guard named a behaviour it did not check.
    callSite: /assessRun\s*\(/,
    consumerCallSite: /clearRunArtifacts\s*\(/,
  },
  {
    id: 'transport-mirror-classification',
    why:
      'mutes the console TWIN of an already-muted dev-asset transport failure, ' +
      'and refuses to mute a dyn-import crash without same-module corroboration (D1)',
    // The classifier is now single-sourced in its own module, so parity for it
    // is by CONSTRUCTION — this only checks each crawl copy actually uses it.
    module: 'finding-classify.mjs',
    callSite: /classifyAll\s*\(/,
  },
]

/** The copies that are actually EXECUTED by some npm script. A dead copy is not
 *  checked — it is deleted (CODING_GUIDELINES §15). */
export const LIVE_COPIES = [
  { id: 'sdk/runtime-health', file: 'sdk/packages/gallery/scripts/runtime-health.mjs' },
  { id: 'sdk/gate-ui', file: 'sdk/packages/gallery/scripts/gate-ui.mjs' },
  { id: 'desktop/runtime-health', file: 'src-app/desktop/ui/scripts/runtime-health.mjs' },
  { id: 'desktop/gate-ui', file: 'src-app/desktop/ui/scripts/gate-ui.mjs' },
]

/** Which cores each copy is REQUIRED to carry. `gate-ui` drives the crawl and
 *  consumes its manifest; `runtime-health` performs the crawl and produces it. */
export const REQUIRED = {
  'sdk/runtime-health': ['host-lock', 'run-manifest', 'run-validity', 'transport-mirror-classification'],
  'sdk/gate-ui': ['host-lock', 'run-manifest', 'run-validity'],
  'desktop/runtime-health': ['host-lock', 'run-manifest', 'run-validity', 'transport-mirror-classification'],
  'desktop/gate-ui': ['host-lock', 'run-manifest', 'run-validity'],
}

/** `runtime-health` PRODUCES the manifest/verdict; `gate-ui` CONSUMES them. */
export const ROLE = {
  'sdk/runtime-health': 'producer',
  'desktop/runtime-health': 'producer',
  'sdk/gate-ui': 'consumer',
  'desktop/gate-ui': 'consumer',
}

/** Pure core — exported so the test can drive it against mutated fixtures. */
export function checkParity(readFile, copies = LIVE_COPIES, required = REQUIRED, roles = ROLE) {
  const violations = []
  for (const copy of copies) {
    const src = readFile(copy.file)
    if (src == null) {
      violations.push(`${copy.id}: expected harness copy is MISSING at ${copy.file}`)
      continue
    }
    for (const coreId of required[copy.id] ?? []) {
      const core = CORES.find(c => c.id === coreId)
      const imports = src.includes(core.module)
      const site =
        roles[copy.id] === 'consumer' && core.consumerCallSite
          ? core.consumerCallSite
          : core.callSite
      const calls = site.test(src)
      if (!imports || !calls)
        violations.push(
          `${copy.id} (${copy.file}) is missing the "${core.id}" core — ` +
            `${!imports ? `it does not import ${core.module}` : `it imports ${core.module} but never calls ${site.source}`}. ` +
            `That core ${core.why}. A fix that lands in one harness copy and not the others ` +
            `is the drift that made this defect class survive repeated fixing.`,
        )
    }
  }
  return violations
}

const isMain =
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith('check-harness-parity.mjs')

if (isMain) {
  const rootArg = process.argv.indexOf('--root')
  // Default: this file lives at <root>/sdk/packages/gallery/scripts/.
  const root =
    rootArg >= 0 ? path.resolve(process.argv[rootArg + 1]) : path.resolve(HERE, '../../../..')
  const readFile = rel => {
    try {
      return fs.readFileSync(path.join(root, rel), 'utf-8')
    } catch {
      return null
    }
  }
  const violations = checkParity(readFile)
  if (violations.length) {
    console.error(`harness parity: ${violations.length} violation(s)\n`)
    for (const v of violations) console.error(`  ✗ ${v}\n`)
    process.exit(1)
  }
  console.log(
    `harness parity: OK — ${LIVE_COPIES.length} live copies carry all ${CORES.length} behavioural cores.`,
  )
}
