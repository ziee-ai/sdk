import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import { collationKey, compareNatural } from './collate.ts'

// ---------------------------------------------------------------------------
// The KEY, not only the ordering. An ordering assertion passes for many wrong
// keys, so the fold is asserted directly.
// ---------------------------------------------------------------------------

test('collationKey folds case, accents and whitespace', () => {
  assert.equal(collationKey('Édith'), 'edith')
  assert.equal(collationKey('EDITH'), 'edith')
  assert.equal(collationKey('  Two   Words '), 'two words')
  assert.equal(collationKey('Åsa'), 'asa')
  assert.equal(collationKey('Łódź'), 'lodz')
})

test('collationKey EXPANDS the multi-letter folds, as ICU does', () => {
  // `Æther` collates as `Aether`, not as `Ather` — a strictly 1:1 table gets
  // exactly these characters wrong.
  assert.equal(collationKey('Æther'), 'aether')
  assert.equal(collationKey('Œuvre'), 'oeuvre'.toLowerCase())
  assert.equal(collationKey('Straße'), 'strasse')
})

test('a character outside the table survives verbatim (lower-cased)', () => {
  assert.equal(collationKey('東京'), '東京')
  assert.equal(collationKey('Привет'), 'привет')
})

// ---------------------------------------------------------------------------
// The two `localeCompare` option flags this replaced.
// ---------------------------------------------------------------------------

test('numeric: digit runs compare as numbers, not lexicographically', () => {
  assert.ok(compareNatural('Item 2', 'Item 10') < 0)
  assert.ok(compareNatural('Item 10', 'Item 2') > 0)
  assert.ok(compareNatural('ch-9', 'ch-10') < 0)
})

test('numeric: a run longer than 2^53 still compares exactly', () => {
  const a = `id-${'9'.repeat(40)}`
  const b = `id-1${'0'.repeat(40)}`
  // 40 nines < 41 digits. `parseInt` would make both `Infinity` and tie.
  assert.ok(compareNatural(a, b) < 0)
})

test('numeric: leading zeros do not change the number', () => {
  assert.equal(compareNatural('v007', 'v7') !== 0, true) // total, but…
  assert.ok(compareNatural('v007', 'v8') < 0) // …7 still sorts before 8
  assert.ok(compareNatural('v010', 'v9') > 0)
})

test('sensitivity base: case and accents do not separate equals', () => {
  assert.ok(compareNatural('apple', 'Banana') < 0)
  assert.ok(compareNatural('Banana', 'apple') > 0)
  assert.ok(compareNatural('Edith', 'edward') < 0) // fold, then compare
  assert.ok(compareNatural('Édith', 'Edward') < 0)
})

// ---------------------------------------------------------------------------
// The contract `Array.prototype.sort` needs.
// ---------------------------------------------------------------------------

test('the ordering is TOTAL — folded ties break on raw code points', () => {
  assert.notEqual(compareNatural('resume', 'résumé'), 0)
  assert.equal(compareNatural('résumé', 'resume'), -compareNatural('resume', 'résumé'))
})

test('reflexive and antisymmetric over a mixed sample', () => {
  const sample = ['Édith', 'edith', 'Item 2', 'Item 10', '東京', 'Æther', '', '  ', 'Åsa', 'zoo']
  for (const a of sample) {
    assert.equal(compareNatural(a, a), 0, `reflexive: ${a}`)
    for (const b of sample) {
      const ab = compareNatural(a, b)
      const ba = compareNatural(b, a)
      // `+ 0` rather than `Math.sign(ab) === -Math.sign(ba)`: `-Math.sign(0)`
      // is `-0`, which `assert.equal` distinguishes from `0`.
      assert.equal(Math.sign(ab) + Math.sign(ba), 0, `antisymmetric: ${a} / ${b}`)
    }
  }
})

test('sorting is stable across runs and independent of input order', () => {
  const words = ['Zoe', 'édith', 'Adam', 'Item 10', 'Item 2', 'Ångström', 'adam']
  const once = [...words].sort(compareNatural)
  const twice = [...words].reverse().sort(compareNatural)
  assert.deepEqual(
    once.map(w => collationKey(w)),
    twice.map(w => collationKey(w)),
  )
})

// ---------------------------------------------------------------------------
// The property the whole module exists for: no ICU is consulted, so an engine
// without it answers identically. Proved by DELETING `Intl` and re-running —
// which `localeCompare` would survive while answering differently, so the
// second half (a comparison against a recorded expectation) is what has teeth.
// ---------------------------------------------------------------------------

test('the answer does not change when Intl is removed from the realm', () => {
  const words = ['Édith', 'edward', 'Item 10', 'Item 2', 'Æther', 'Straße', 'zoo', 'Åsa']
  const before = [...words].sort(compareNatural)

  const g = globalThis as { Intl?: unknown }
  const saved = g.Intl
  delete g.Intl
  try {
    const after = [...words].sort(compareNatural)
    assert.deepEqual(after, before)
    // …and it is the order we RECORDED, so a comparator that quietly became a
    // no-op cannot pass this by returning the input unchanged in both realms.
    assert.deepEqual(after, [
      'Æther',
      'Åsa',
      'Édith',
      'edward',
      'Item 2',
      'Item 10',
      'Straße',
      'zoo',
    ])
  } finally {
    g.Intl = saved
  }
})

// ---------------------------------------------------------------------------
// THE GUARD. Everything above proves the comparator BEHAVES; this proves it
// does not consult ICU at all — which is the property a consumer that
// server-renders actually depends on, and the one a well-meant "just use
// localeCompare, it's simpler" edit would silently undo.
//
// A behavioural test cannot catch that regression: deleting `globalThis.Intl`
// leaves `String.prototype.localeCompare` in place, answering (differently, and
// only on inputs a fold table and a collator disagree about). So the guard is a
// source scan, and it is red for the exact edit it exists to refuse.
// ---------------------------------------------------------------------------

const HERE = path.dirname(new URL(import.meta.url).pathname)

test('neither the collator nor the table core names an ICU entry point', () => {
  for (const file of ['collate.ts', 'table-view-core.ts']) {
    const src = fs.readFileSync(path.join(HERE, file), 'utf8')
    // Comments are allowed to NAME the call they replaced — the prose above and
    // in `table-view-core.ts` does exactly that — so comment lines are stripped
    // before the scan rather than the tokens being made vaguer.
    const code = src
      .split('\n')
      .filter(line => !/^\s*(\/\/|\*|\/\*)/.test(line))
      .join('\n')
    for (const token of ['localeCompare', 'toLocale', 'Intl.']) {
      assert.equal(
        code.includes(token),
        false,
        `${file} must not call ${token} — it renders a DOM-tree order and must be ` +
          'identical in a runtime with no ICU. See collate.ts.',
      )
    }
  }
})
