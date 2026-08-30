// Deterministic, ICU-free collation for the kit's headless table core.
//
// ## Why this exists
//
// `table-view-core.ts` used to order text cells with
// `localeCompare(a, b, undefined, { numeric: true, sensitivity: 'base' })`.
// That is an ICU call, and `Array.prototype.sort` over it produces a DOM-TREE
// ORDER — not a string. An engine without ICU behind `localeCompare` (QuickJS,
// QuickJS-ng, Hermes without `intl`, any `--without-intl` Node) still HAS the
// method: it answers, it just answers differently. So the failure mode is not a
// throw, it is two halves of one page disagreeing about where `Édith` sorts,
// which surfaces as a React hydration error rather than as anything a reviewer
// would notice.
//
// A consuming app that server-renders therefore cannot use the kit's table
// unless the comparator is the SAME FUNCTION on both sides. It is data + plain
// arithmetic here, so it is.
//
// ## What it preserves from the call it replaces
//
//   `numeric: true`         — `Item 2` sorts before `Item 10`.
//   `sensitivity: 'base'`   — case AND accent insensitive: `a` ≡ `A` ≡ `á` ≡ `Á`.
//
// ## What it deliberately does NOT reproduce
//
// Full CLDR collation: punctuation weighting, script reordering, locale-specific
// letter order (Swedish `ä` after `z`, Czech `ch` between `h` and `i`), and the
// tailorings a real collator applies. Outside the fold table below — CJK,
// Cyrillic, Greek, emoji — comparison falls through to code points, which is
// deterministic and stable but not culturally ordered.
//
// That trade is the honest one for a table: an order every reader can rely on,
// identical in every runtime, beats an ICU-faithful order that differs between
// the server render and the client hydration of the same page.
//
// Kept dependency-free and erasable-TS for the same reason as its consumer:
// `node --test` type-strips and imports it with no bundler and no DOM.

/**
 * `<accented><base>` pairs, written as ONE string so the table stays small
 * enough to read in a review — a 200-entry object literal is not.
 *
 * Coverage: Latin-1 Supplement + Latin Extended-A, which is the realistic
 * accent range for the text a table cell holds.
 */
const FOLD_PAIRS =
  'ÀAÁAÂAÃAÄAÅAÇCÈEÉEÊEËEÌIÍIÎIÏIÑNÒOÓOÔOÕOÖOØOÙUÚUÛUÜUÝYàaáaâaãaäaåaçcèeéeêeëeìiíiîiïiñnòoóoôoõoöoøoùuúuûuüuýyÿy' +
  'ĀAāaĂAăaĄAąaĆCćcĈCĉcĊCċcČCčcĎDďdĐDđdĒEēeĔEĕeĖEėeĘEęeĚEěeĜGĝgĞGğgĠGġgĢGģgĤHĥhĦHħh' +
  'ĨIĩiĪIīiĬIĭiĮIįiİIıiĴJĵjĶKķkĹLĺlĻLļlĽLľlĿLŀlŁLłlŃNńnŅNņnŇNňnŌOōoŎOŏoŐOőoŔRŕrŖRŗrŘRřr' +
  'ŚSśsŜSŝsŞSşsŠSšsŢTţtŤTťtŦTŧtŨUũuŪUūuŬUŭuŮUůuŰUűuŲUųuŴWŵwŶYŷyŸYŹZźzŻZżzŽZžz'

/**
 * Characters that fold to MORE THAN ONE letter. ICU expands these rather than
 * mapping them to a single base — `Æther` collates as `Aether`, not as `Ather`
 * — so a strictly 1:1 table gets exactly the characters a European title
 * reaches for measurably wrong.
 */
const FOLD_EXPANSIONS: ReadonlyArray<readonly [string, string]> = [
  ['Æ', 'AE'], ['æ', 'ae'],
  ['Œ', 'OE'], ['œ', 'oe'],
  ['ß', 'ss'],
  ['Þ', 'TH'], ['þ', 'th'],
  ['Ð', 'D'], ['ð', 'd'],
  ['Ø', 'O'], ['ø', 'o'],
]

const FOLD = new Map<string, string>()
for (let i = 0; i < FOLD_PAIRS.length; i += 2) FOLD.set(FOLD_PAIRS[i], FOLD_PAIRS[i + 1])
for (const [from, to] of FOLD_EXPANSIONS) FOLD.set(from, to)

/**
 * The comparison key: accent-folded, case-folded, whitespace-collapsed.
 *
 * `toLowerCase`, never `toLocaleLowerCase` — the locale-aware variant is the
 * Turkish-dotless-i trap and is host-dependent, which is the entire class of
 * bug this module removes.
 *
 * Exported so a test can assert the KEY rather than only the ordering: an
 * ordering assertion passes for many wrong keys.
 */
export function collationKey(value: string): string {
  let out = ''
  for (const ch of value.trim()) out += FOLD.get(ch) ?? ch
  return out.toLowerCase().replace(/\s+/g, ' ')
}

const isDigit = (code: number): boolean => code >= 48 && code <= 57

/** Strip leading zeros but keep at least one digit, so `007` compares as `7`. */
const significant = (run: string): string => {
  let i = 0
  while (i < run.length - 1 && run.charCodeAt(i) === 48) i += 1
  return run.slice(i)
}

/**
 * Compare two already-folded keys, treating runs of ASCII digits as numbers.
 *
 * Digit runs are compared by SIGNIFICANT LENGTH first and only then
 * lexicographically, which is exact for arbitrarily long runs — `parseInt`
 * would lose precision past 2^53 and turn a 40-digit id into `Infinity`.
 */
function compareKeys(x: string, y: string): number {
  let i = 0
  let j = 0
  while (i < x.length && j < y.length) {
    const cx = x.charCodeAt(i)
    const cy = y.charCodeAt(j)
    if (isDigit(cx) && isDigit(cy)) {
      const si = i
      const sj = j
      while (i < x.length && isDigit(x.charCodeAt(i))) i += 1
      while (j < y.length && isDigit(y.charCodeAt(j))) j += 1
      const nx = significant(x.slice(si, i))
      const ny = significant(y.slice(sj, j))
      if (nx.length !== ny.length) return nx.length < ny.length ? -1 : 1
      if (nx !== ny) return nx < ny ? -1 : 1
      continue
    }
    if (cx !== cy) return cx < cy ? -1 : 1
    i += 1
    j += 1
  }
  if (i < x.length) return 1
  if (j < y.length) return -1
  return 0
}

/**
 * Order two strings. Returns the `Array.prototype.sort` contract (-1 / 0 / 1).
 *
 * TOTAL: keys that tie fall back to a raw code-point comparison, so `resume`
 * and `résumé` — and `Item 1` and `Item 01` — get a stable order rather than an
 * arbitrary one that depends on the input sequence.
 */
export function compareNatural(a: string, b: string): number {
  const c = compareKeys(collationKey(a), collationKey(b))
  if (c !== 0) return c
  if (a < b) return -1
  if (a > b) return 1
  return 0
}
