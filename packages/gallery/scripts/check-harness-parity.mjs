/**
 * HARNESS PARITY GUARD — the gallery crawl harness exists in more than one tree,
 * and those copies have DRIFTED. This refuses a tree where a behavioural core is
 * present in one live copy and missing from another.
 *
 * ## What this proves — and what it CANNOT
 *
 * It asserts each declared copy IMPORTS the shared behavioural module and CALLS
 * its entry point. That is **WIRING, not LOGIC**: a copy can keep
 * `verifyRunManifest(...)` and hardcode `const usable = { ok: true }` and this
 * guard stays green — while the gate fails OPEN. Do not read a pass here as
 * verification that a copy behaves correctly.
 *
 * That limit is why the harness was UNIFIED rather than more heavily guarded.
 * Both ziee ui workspaces now run ONE implementation (these very scripts) with
 * their own `gallery.config.json`; the desktop forks were deleted. Parity is
 * true by construction, and the check that actually carries the invariant is
 * the consumer test's "no workspace re-forks the harness" assertion plus its
 * content-based discovery walk — not this file.
 *
 * Three rounds of blind audit each found a fresh evasion of an earlier,
 * regex-based version of this guard, and the lifecycle validator's own tripwire
 * ruled the loop non-converging: a hand-written static-analysis guard standing
 * in for a behavioural test has an unbounded evasion space. Kept in reduced
 * form for the case it still covers cheaply (a declared copy that silently
 * stops importing a core), and it names that scope in its own output.
 *
 * Exit 0 = every live copy carries every core. Exit 1 = a copy is missing one.
 *
 * Run (from the app's ui/ cwd, so gallery.config.json resolves):
 *   node node_modules/@ziee/gallery/scripts/check-harness-parity.mjs
 */
import fs from 'node:fs'
import path from 'node:path'
import { resolveGalleryConfig } from './lib/gallery-config.mjs'

/**
 * Each core: the shared module that owns the behaviour, plus a call site that
 * proves the copy actually USES it (an import alone can be dead).
 */
export const CORES = [
  {
    id: 'host-lock',
    roles: ['producer', 'consumer'],
    why: 'serializes crawls across worktrees on one host (D3)',
    module: 'host-lock.mjs',
    // Either entry point: the crawl wraps itself in withHostLock; the gate takes
    // the lock around only its shared-resource phase via acquire().
    callSite: /withHostLock\s*\(|acquire\s*\(\s*\{/,
  },
  {
    id: 'run-manifest',
    roles: ['producer', 'consumer'],
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
    roles: ['producer', 'consumer'],
    why: 'VOIDs a run whose origin died or whose findings are mostly transport noise (D1)',
    module: 'run-validity.mjs',
    // Specifically assessRun — NOT an alternation with clearRunArtifacts. The
    // looser form let a copy delete the entire void-assessment and stay green on
    // the unrelated clear call, i.e. the guard named a behaviour it did not check.
    callSite: /assessRun\s*\(/,
    consumerCallSite: /clearRunArtifacts\s*\(/,
  },
  {
    id: 'origin-watchdog',
    roles: ['producer'],
    why:
      'watches the dev server for the whole crawl, so a run whose ORIGIN DIED ' +
      'mid-crawl is VOIDed rather than reported as a clean pass (D1)',
    module: 'run-validity.mjs',
    // run-validity's `why` names TWO behaviours — "origin died OR findings are
    // mostly transport noise" — but only `assessRun` was pinned, so the entire
    // origin-death half could be dropped from a copy with every suite green.
    // That is the guard naming a behaviour it does not check, which this file's
    // own comments condemn. Pinned as its own core rather than widened into an
    // alternation, because an alternation lets either half satisfy both.
    callSite: /watchOrigin\s*\(|originAlive\s*\(/,
  },
  {
    id: 'transport-mirror-classification',
    roles: ['producer'],
    why:
      'mutes the console TWIN of an already-muted dev-asset transport failure, ' +
      'and refuses to mute a dyn-import crash without same-module corroboration (D1)',
    // The classifier is now single-sourced in its own module, so parity for it
    // is by CONSTRUCTION — this only checks each crawl copy actually uses it.
    module: 'finding-classify.mjs',
    callSite: /classifyAll\s*\(/,
  },
  {
    id: 'console-classification',
    roles: ['producer'],
    why:
      'classifies a console message by TEXT, not by channel — React 19 emits ' +
      'developer warnings on console.error, so a channel-derived severity gated ' +
      'every React warning as HIGH against the harness\'s own MEDIUM taxonomy',
    module: 'finding-classify.mjs',
    // The crawl must DELEGATE the decision. A copy that reconstructs its own
    // `m.type() === 'error' ? … : …` branch re-introduces the version coupling
    // this core exists to remove, and would not match.
    callSite: /classifyConsoleMessage\s*\(/,
  },
]

/**
 * WHICH copies exist is the CONSUMER's fact, not this package's.
 *
 * This guard used to hardcode `src-app/desktop/ui/...` — ziee's layout — inside
 * the shared `@ziee/gallery` package, which made the guard and its own test
 * red-by-construction in a standalone sdk checkout or in any second consumer.
 * That is the SAME defect class the guard exists to prevent (shared tooling that
 * assumes one consumer), so the paths are now supplied the way every other
 * generic gallery script takes its anchors: from the app's `gallery.config.json`
 * (cf. `srcDir` / `extraTrees` / `kitTestIds` in `lib/gallery-config.mjs`).
 *
 * `harnessCopies` is either
 *   - a STRING: a path (relative to the app's cwd) to a JSON manifest
 *     `{ "copies": [ … ] }` whose own `file` paths are relative to the MANIFEST,
 *     so several workspaces can share ONE declaration instead of duplicating it
 *     (duplicating the list would re-create the drift this guard checks for); or
 *   - an inline ARRAY of the same entries, with `file` relative to cwd.
 *
 * Each entry: `{ id, file, role: 'producer'|'consumer', cores: [coreId, …] }`.
 * Absent/empty ⇒ nothing to check (a standalone package legitimately has no
 * consumer copies) and the guard exits 0 saying so.
 */

/** The cores a copy must carry, derived from its ROLE (not from the manifest). */
export const requiredCores = copy => CORES.filter(c => c.roles.includes(copy.role))

/** Pure core — drives against whatever `copies` it is handed. `readFile(file)`
 *  returns the source or `null`. */
export function checkParity(readFile, copies) {
  const violations = []

  // --- declaration integrity -----------------------------------------------
  // WHICH cores a copy must carry is derived from its ROLE, by this package —
  // it is NOT read from the consumer's manifest. An earlier version let the
  // manifest list the cores per copy, which reintroduced the exact bug the guard
  // exists to catch: dropping a core from ONE copy's list made that copy stop
  // being checked, silently, while the CLI still printed "all N cores". The
  // consumer declares only WHERE its copies are and WHAT each one is.
  const ROLES = new Set(['producer', 'consumer'])
  for (const copy of copies) {
    if (!ROLES.has(copy.role))
      violations.push(
        `${copy.id}: has role "${copy.role ?? '(none)'}" — must be one of ` +
          `${[...ROLES].join(' | ')}. The role determines which cores the copy ` +
          `must carry, so an unrecognised one would check nothing.`,
      )
    if (copy.cores)
      violations.push(
        `${copy.id}: declares "cores" — remove it. Required cores are derived ` +
          `from the role by @ziee/gallery; a per-copy list is how a core silently ` +
          `stops being checked.`,
      )
  }
  const ids = copies.map(c => c.id)
  const files = copies.map(c => c.file)
  for (const dup of [...new Set(ids.filter((x, i) => ids.indexOf(x) !== i))])
    violations.push(`duplicate copy id "${dup}" — each live copy must be listed once.`)
  for (const dup of [...new Set(files.filter((x, i) => files.indexOf(x) !== i))])
    violations.push(
      `two entries point at the same file "${dup}" — one source would be read ` +
        `twice and a real copy would go unchecked.`,
    )
  // Every core must be exercised by at least one copy of a role that carries it.
  for (const core of CORES)
    if (!copies.some(c => core.roles.includes(c.role)))
      violations.push(
        `core "${core.id}" is required by no declared copy (no ${core.roles.join('/')} ` +
          `is listed), so it is never checked. That core ${core.why}.`,
      )

  if (violations.length) return violations

  for (const copy of copies) {
    const src = readFile(copy.file)
    if (src == null) {
      violations.push(`${copy.id}: expected harness copy is MISSING at ${copy.file}`)
      continue
    }
    for (const core of requiredCores(copy)) {
      const imports = src.includes(core.module)
      const site =
        copy.role === 'consumer' && core.consumerCallSite
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

/**
 * Resolve the consumer's declared copies into entries whose `file` is ABSOLUTE.
 * Returns `{ copies, source }`; `source` names where the declaration came from
 * (for the operator-facing log), or `null` when nothing is configured.
 */
export function resolveHarnessCopies(cfg, io = fs) {
  const spec = cfg.harnessCopies
  if (!spec || (Array.isArray(spec) && spec.length === 0))
    return { copies: [], source: null }

  let entries
  let base
  let source
  if (typeof spec === 'string') {
    source = path.resolve(cfg.__cwd, spec)
    let raw
    try {
      raw = io.readFileSync(source, 'utf8')
    } catch (e) {
      // A configured-but-unreadable manifest must be an ERROR, never an empty
      // list: silently checking nothing is how a gate stops gating.
      throw new Error(`[harness-parity] harnessCopies manifest not readable at ${source}: ${e.message}`)
    }
    let parsed
    try {
      parsed = JSON.parse(raw)
    } catch (e) {
      throw new Error(`[harness-parity] failed to parse ${source}: ${e.message}`)
    }
    entries = parsed.copies
    base = path.dirname(source)
    if (!Array.isArray(entries))
      throw new Error(`[harness-parity] ${source} has no "copies" array`)
  } else {
    entries = spec
    base = cfg.__cwd
    source = cfg.__configPath
  }

  return {
    source,
    copies: entries.map(e => ({ ...e, file: path.resolve(base, e.file) })),
  }
}

const isMain =
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith('check-harness-parity.mjs')

if (isMain) {
  const { copies, source } = resolveHarnessCopies(resolveGalleryConfig())
  if (!copies.length) {
    console.log(
      'harness parity: no consumer harness copies configured ' +
        '(set "harnessCopies" in gallery.config.json) — nothing to check.',
    )
    process.exit(0)
  }
  const readFile = abs => {
    try {
      return fs.readFileSync(abs, 'utf-8')
    } catch {
      return null
    }
  }
  const violations = checkParity(readFile, copies)
  if (violations.length) {
    console.error(`harness parity: ${violations.length} violation(s)\n`)
    for (const v of violations) console.error(`  ✗ ${v}\n`)
    process.exit(1)
  }
  const checked = new Set(copies.flatMap(c => requiredCores(c).map(x => x.id)))
  console.log(
    `harness parity: OK — ${copies.length} live copies (from ${path.relative(process.cwd(), source)}) ` +
      `carry all ${checked.size} behavioural cores: ${[...checked].sort().join(', ')}.\n` +
      `  note: this proves each copy WIRES each core (imports + calls it). It does ` +
      `NOT prove the copy's logic is correct — a copy can keep a call and hardcode ` +
      `its result. Single-sourcing the harness, not this guard, is what makes parity true.`,
  )
}
