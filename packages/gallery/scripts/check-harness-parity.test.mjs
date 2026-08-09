/**
 * TEST — the parity guard's ENGINE, driven against synthetic fixtures.
 *
 * A guard that merely passes today proves nothing; the invariant is that it goes
 * RED when a live copy loses a core. So every case below MUTATES a copy and
 * asserts the guard fires, with a positive control that the unmutated set passes.
 *
 * **Nothing here names a consumer app's paths.** This file used to read ziee's
 * real tree (`src-app/desktop/ui/...`) from inside the shared package, so it was
 * red-by-construction in a standalone sdk checkout — the same "shared tooling
 * assumes one consumer" defect the guard exists to prevent. The real-tree
 * acceptance case now lives with the consumer that owns those paths
 * (`src-app/ui/scripts/check-harness-parity.consumer.test.mjs`); what belongs
 * HERE is the engine and the config contract.
 *
 * Run: node --test scripts/check-harness-parity.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { CORES, checkParity, requiredCores, resolveHarnessCopies } from './check-harness-parity.mjs'

// A minimal fake tree: one producer + one consumer, each carrying every core.
const PRODUCER_SRC = `
import { withHostLock } from './lib/host-lock.mjs'
import { writeRunManifest, assessRun, watchOrigin } from './lib/run-validity.mjs'
import { classifyAll, classifyConsoleMessage } from './lib/finding-classify.mjs'
await withHostLock(async () => {
  watchOrigin(o); writeRunManifest(x); assessRun(y); classifyAll(z); classifyConsoleMessage(a, b, c)
})
`
const CONSUMER_SRC = `
import { acquire } from './lib/host-lock.mjs'
import { verifyRunManifest, clearRunArtifacts } from './lib/run-validity.mjs'
acquire({ scope: 'gate' }); verifyRunManifest(m); clearRunArtifacts(d)
`
// The consumer declares only WHERE and WHAT — never which cores.
const COPIES = [
  { id: 'app/runtime-health', file: '/fake/runtime-health.mjs', role: 'producer' },
  { id: 'app/gate-ui', file: '/fake/gate-ui.mjs', role: 'consumer' },
]
const tree = {
  '/fake/runtime-health.mjs': PRODUCER_SRC,
  '/fake/gate-ui.mjs': CONSUMER_SRC,
}
const readFrom = t => f => (f in t ? t[f] : null)

test('positive control — a tree carrying every core passes', () => {
  const violations = checkParity(readFrom(tree), COPIES)
  assert.deepEqual(violations, [], violations.join('\n'))
})

test('RED when a copy loses a core (the fix landed in one place only)', () => {
  for (const core of CORES) {
    const mutated = {
      ...tree,
      '/fake/runtime-health.mjs': PRODUCER_SRC.replaceAll(core.module, 'REMOVED'),
    }
    const violations = checkParity(readFrom(mutated), COPIES)
    assert.ok(
      violations.some(v => v.includes(`"${core.id}"`)),
      `losing ${core.module} must flag the "${core.id}" core; got:\n${violations.join('\n')}`,
    )
    assert.ok(
      violations.every(v => v.includes('app/runtime-health')),
      'and ONLY the mutated copy — the untouched one must stay clean',
    )
  }
})

test('an IMPORT without a CALL is still a violation (dead wiring)', () => {
  // The subtler drift: the import line is copied but never called, so a naive
  // "does it import the module" guard passes while the behaviour is absent
  // (CODING_GUIDELINES §15 — dead code is unfinished work).
  const mutated = {
    ...tree,
    '/fake/runtime-health.mjs': PRODUCER_SRC.replaceAll('withHostLock(async', 'notCalled(async'),
  }
  const violations = checkParity(readFrom(mutated), COPIES)
  assert.ok(
    violations.some(v => /imports host-lock\.mjs but never calls/.test(v)),
    'the guard must distinguish "not imported" from "imported but unused"',
  )
})

test('a producer cannot satisfy a CONSUMER core, and vice-versa', () => {
  // One alternation covering both sides would let a producer-only copy satisfy
  // the consumer half; each role names the call IT must make.
  const swapped = { ...tree, '/fake/gate-ui.mjs': PRODUCER_SRC }
  const violations = checkParity(readFrom(swapped), COPIES)
  assert.ok(
    violations.some(v => v.includes('app/gate-ui') && /run-manifest/.test(v)),
    `a consumer carrying only producer calls must fail; got:\n${violations.join('\n')}`,
  )
})

test('a MISSING copy is reported, not silently skipped', () => {
  const violations = checkParity(readFrom({ '/fake/gate-ui.mjs': CONSUMER_SRC }), COPIES)
  assert.ok(violations.some(v => /is MISSING at/.test(v)))
})

test('a manifest CANNOT under-declare cores — the whole mechanism is refused', () => {
  // THE regression test for the round-3 miss. When the manifest listed cores,
  // dropping one from a SINGLE copy (verbatim the historical failure: "the fix
  // landed in the sdk copy and the desktop copy was forgotten") left that copy
  // unchecked while the guard printed "all N cores". Required cores are now
  // derived from the role, and a leftover per-copy list is itself a violation.
  const under = [{ ...COPIES[0], cores: ['host-lock'] }, COPIES[1]]
  const violations = checkParity(readFrom(tree), under)
  assert.ok(
    violations.some(v => /declares "cores" — remove it/.test(v)),
    `a per-copy core list must be refused; got:\n${violations.join('\n')}`,
  )
})

test('required cores are ROLE-derived, and a producer carries strictly more', () => {
  const prod = requiredCores({ role: 'producer' }).map(c => c.id)
  const cons = requiredCores({ role: 'consumer' }).map(c => c.id)
  assert.ok(prod.includes('console-classification'), 'producer must carry the classifier cores')
  assert.ok(!cons.includes('console-classification'), 'a gate-ui does not crawl')
  for (const id of cons) assert.ok(prod.includes(id), `producer must also carry ${id}`)
  assert.deepEqual([...new Set([...prod, ...cons])].sort(), CORES.map(c => c.id).sort(),
    'every declared core must be required by some role — a core nobody carries is decoration')
})

test('a core required by no DECLARED copy is an error', () => {
  // e.g. an app that lists only a gate-ui: the producer-only cores are checked
  // by nobody, and the guard must say so rather than report success.
  const violations = checkParity(readFrom(tree), [COPIES[1]])
  assert.ok(violations.some(v => /core "console-classification" is required by no declared copy/.test(v)))
})

test('duplicate ids, and two entries naming the SAME file, are errors', () => {
  const dupFile = [COPIES[0], { ...COPIES[1], file: COPIES[0].file }]
  assert.ok(checkParity(readFrom(tree), dupFile).some(v => /same file/.test(v)),
    'one source read twice means a real copy goes unchecked')
  const dupId = [COPIES[0], { ...COPIES[1], id: COPIES[0].id }]
  assert.ok(checkParity(readFrom(tree), dupId).some(v => /duplicate copy id/.test(v)))
})

test('an unknown ROLE is an error — it decides which cores are required', () => {
  const bad = [{ ...COPIES[1], role: 'CONSUMER' }, COPIES[0]]
  const violations = checkParity(readFrom(tree), bad)
  assert.ok(violations.some(v => /has role "CONSUMER"/.test(v)))
})

test('declaration errors are COLLECTED, not early-returned on the first', () => {
  // One typo must not suppress the rest of the report and send the operator
  // round the loop twice.
  const bad = COPIES.map(c => ({ ...c, role: 'nope', cores: ['x'] }))
  const violations = checkParity(readFrom(tree), bad)
  assert.ok(violations.some(v => /has role/.test(v)))
  assert.ok(violations.some(v => /declares "cores"/.test(v)))
  assert.ok(violations.length >= 4, `expected several, got ${violations.length}`)
})

// ---------------------------------------------------------------------------
// The CONFIG contract — how a consumer supplies its own paths.
// ---------------------------------------------------------------------------
test('no harnessCopies configured ⇒ nothing to check (a standalone package)', () => {
  assert.deepEqual(resolveHarnessCopies({ __cwd: '/app', harnessCopies: null }), {
    copies: [],
    source: null,
  })
  assert.deepEqual(resolveHarnessCopies({ __cwd: '/app', harnessCopies: [] }).copies, [])
})

test('an inline array resolves file paths against the app cwd', () => {
  const { copies } = resolveHarnessCopies({
    __cwd: '/app/ui',
    __configPath: '/app/ui/gallery.config.json',
    harnessCopies: [{ id: 'a', file: '../scripts/x.mjs', role: 'producer', cores: [] }],
  })
  assert.equal(copies[0].file, '/app/scripts/x.mjs')
})

test('a manifest STRING resolves file paths against the MANIFEST, not cwd', () => {
  // This is what lets several workspaces at different depths share ONE
  // declaration; resolving against each cwd would force a per-workspace copy of
  // the list — the very drift the guard checks for.
  const io = {
    readFileSync: p => {
      assert.equal(p, '/repo/harness.json')
      return JSON.stringify({ copies: [{ id: 'a', file: 'sdk/x.mjs', role: 'producer' }] })
    },
  }
  const { copies, source } = resolveHarnessCopies(
    { __cwd: '/repo/app/ui', harnessCopies: '../../harness.json' },
    io,
  )
  assert.equal(source, '/repo/harness.json')
  assert.equal(copies[0].file, '/repo/sdk/x.mjs')
})

test('a configured-but-unreadable manifest THROWS — it never checks nothing', () => {
  const io = {
    readFileSync: () => {
      throw new Error('ENOENT')
    },
  }
  assert.throws(
    () => resolveHarnessCopies({ __cwd: '/repo', harnessCopies: 'missing.json' }, io),
    /not readable/,
  )
})

test('a malformed manifest THROWS rather than degrading to an empty check', () => {
  assert.throws(
    () => resolveHarnessCopies({ __cwd: '/r', harnessCopies: 'm.json' }, { readFileSync: () => 'not json' }),
    /failed to parse/,
  )
  assert.throws(
    () => resolveHarnessCopies({ __cwd: '/r', harnessCopies: 'm.json' }, { readFileSync: () => '{}' }),
    /no "copies" array/,
  )
})

test('every declared core carries the metadata the reporter prints', () => {
  // Guards the guard: a core with no `why` reports a violation that does not say
  // what broke, which is how a red gate gets routed around instead of fixed.
  for (const c of CORES) {
    assert.ok(c.id && c.module, `core ${c.id} must name its module`)
    assert.ok(c.why?.length > 20, `core ${c.id} must explain WHY it matters`)
    assert.ok(c.callSite instanceof RegExp, `core ${c.id} must name a call site`)
    assert.ok(Array.isArray(c.roles) && c.roles.length, `core ${c.id} must name the roles that carry it`)
  }
})

test('the CORES set is PINNED — a core cannot be deleted silently', () => {
  // Round-3/4 both moved WHERE cores are declared, but nothing pinned the set
  // itself: deleting `run-validity` or `transport-mirror-classification` from
  // CORES removed the check from all four copies with every suite green, and the
  // CLI then cheerfully printed "carry all 3 behavioural cores". The two that
  // survived were exactly the two no test named by literal id.
  //
  // Both sides of the earlier "completeness" assertion were derived from CORES,
  // so it could never notice a deletion. This list is written by hand ON PURPOSE:
  // adding or removing a core must be a deliberate edit here, with a reason.
  assert.deepEqual(
    CORES.map(c => c.id).sort(),
    [
      'console-classification',
      'host-lock',
      'origin-watchdog',
      'run-manifest',
      'run-validity',
      'transport-mirror-classification',
    ],
    'the behavioural core set changed — update this list deliberately, and say why',
  )
})

test('the guard STATES its own limit in the text it prints', () => {
  // TEST-40 claimed this and nothing checked it: the note lives in the isMain
  // block, which no unit test executes. Assert the source of the operator-facing
  // line, so the caveat cannot be quietly dropped while the claim survives.
  const src = readFileSync(new URL('./check-harness-parity.mjs', import.meta.url), 'utf8')
  const banner = src.slice(src.indexOf('harness parity: OK'))
  assert.match(banner, /WIRES each core/, 'the success line must say it proves wiring')
  assert.match(
    banner,
    /does NOT prove the copy's logic|NOT prove the copy.s logic/,
    'and must say what it does NOT prove — a wiring check read as verification is the defect',
  )
})
