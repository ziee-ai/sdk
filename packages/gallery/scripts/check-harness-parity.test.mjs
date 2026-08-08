/**
 * TEST-6 [acceptance · INV-6] — the parity guard actually CATCHES the
 * "fix lands in one place and not the others" failure.
 *
 * A guard that merely passes today proves nothing; the invariant is that it goes
 * RED when a live copy loses a core. So every case below MUTATES a copy and
 * asserts the guard fires, with a positive control that the unmutated set passes.
 *
 * Run: node --test scripts/check-harness-parity.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'
import { CORES, LIVE_COPIES, REQUIRED, checkParity } from './check-harness-parity.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const ROOT = path.resolve(HERE, '../../../..')
const realRead = rel => {
  try {
    return fs.readFileSync(path.join(ROOT, rel), 'utf-8')
  } catch {
    return null
  }
}

test('TEST-6 [acceptance INV-6] the REAL tree passes (positive control)', () => {
  const violations = checkParity(realRead)
  assert.deepEqual(
    violations,
    [],
    `every live harness copy must carry every core:\n${violations.join('\n')}`,
  )
})

test('TEST-6b the guard goes RED when a copy loses the host-lock core', () => {
  // Simulate exactly the historical failure: the fix landed in the sdk copy and
  // the desktop copy was forgotten.
  const mutated = rel => {
    const src = realRead(rel)
    if (src == null) return null
    return rel.includes('desktop') ? src.replaceAll('host-lock.mjs', 'REMOVED').replaceAll('withHostLock', 'REMOVED') : src
  }
  const violations = checkParity(mutated)
  assert.ok(violations.length >= 2, 'both desktop copies must be flagged')
  assert.ok(
    violations.every(v => v.includes('desktop')),
    'and ONLY the desktop copies — the sdk copy is untouched',
  )
  assert.ok(violations.some(v => /host-lock/.test(v)))
  assert.ok(
    violations.some(v => /lands in one harness copy and not the others/.test(v)),
    'the message must explain WHY this matters',
  )
})

test('TEST-6c an IMPORT without a CALL is still a violation (dead wiring)', () => {
  // The subtler drift: someone copies the import line but never calls it, so a
  // naive "does it import the module" guard would pass while the behaviour is
  // absent (CODING_GUIDELINES §15 — dead code is unfinished work).
  const mutated = rel => {
    const src = realRead(rel)
    if (src == null) return null
    return rel === 'src-app/desktop/ui/scripts/runtime-health.mjs'
      ? src.replaceAll('withHostLock', 'notCalledAtAll').replaceAll('acquire({', 'notCalledAtAll({')
      : src
  }
  const violations = checkParity(mutated)
  assert.ok(violations.length >= 1)
  assert.ok(
    violations.some(v => /imports host-lock\.mjs but never calls/.test(v)),
    'the guard must distinguish "not imported" from "imported but unused"',
  )
})

test('TEST-6d a MISSING copy is reported, not silently skipped', () => {
  const mutated = rel => (rel.includes('desktop/ui/scripts/gate-ui') ? null : realRead(rel))
  const violations = checkParity(mutated)
  assert.ok(violations.some(v => /is MISSING at/.test(v)))
})

test('TEST-6e each declared core is required by at least one live copy', () => {
  // Guards the guard: a core nobody requires is decoration.
  const requiredIds = new Set(Object.values(REQUIRED).flat())
  for (const c of CORES)
    assert.ok(requiredIds.has(c.id), `core "${c.id}" is required by no copy`)
  for (const copy of LIVE_COPIES)
    assert.ok(REQUIRED[copy.id]?.length, `live copy "${copy.id}" requires no cores`)
})

test('the DEAD ui-local copy is not resurrected', () => {
  assert.equal(
    realRead('src-app/ui/scripts/runtime-health.mjs'),
    null,
    'src-app/ui/scripts/runtime-health.mjs had zero invokers and was deleted; ' +
      'a new copy there is a third divergent harness and must not come back',
  )
})
