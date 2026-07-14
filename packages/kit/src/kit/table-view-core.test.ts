import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  type CoreColumn,
  type TableSelection,
  applyFilter,
  applySort,
  canHideColumn,
  clampWidth,
  compareValues,
  deriveView,
  detectNumericColumns,
  isNumericColumn,
  matchesFilter,
  nextSort,
  serializeSelectionTsv,
  serializeTsv,
} from './table-view-core.ts'

interface Row {
  id: string
  name: string
  qty: string
  note: string
}

const cols: CoreColumn[] = [
  { key: 'name', dataIndex: 'name' },
  { key: 'qty', dataIndex: 'qty' },
  { key: 'note', dataIndex: 'note' },
]

const rows: Row[] = [
  { id: '1', name: 'Banana', qty: '10', note: 'ripe' },
  { id: '2', name: 'apple', qty: '2', note: 'GREEN' },
  { id: '3', name: 'Cherry', qty: '30', note: 'red' },
]

// TEST-1 — default view is a pure identity (backward-compat baseline for the new props)
test('deriveView with no sort/filter returns dataSource unchanged in order', () => {
  const out = deriveView(rows, cols, { sort: null, query: '' })
  assert.deepEqual(out.map(r => r.id), ['1', '2', '3'])
  // filter passthrough returns the same array reference (no needless copy)
  assert.equal(applyFilter(rows, cols, ''), rows)
})

// TEST-2 — numeric vs string ordering + custom sorter override
test('compareValues orders numbers numerically and text via locale', () => {
  assert.ok(compareValues('2', '10') < 0) // numeric, not lexicographic
  assert.ok(compareValues('10', '2') > 0)
  assert.ok(compareValues('apple', 'Banana') < 0) // case-insensitive locale
})

test('a custom sorter overrides the default comparator', () => {
  const custom: CoreColumn[] = [{ key: 'name', dataIndex: 'name', sorter: () => 0 }]
  // sorter returns 0 for all → stable order preserved
  const out = applySort(rows, custom, { key: 'name', dir: 'asc' })
  assert.deepEqual(out.map(r => r.id), ['1', '2', '3'])
})

// TEST-3 — tri-state sort: asc/desc reverse; none restores original order
test('applySort asc/desc reverse each other; null restores original order', () => {
  const asc = applySort(rows, cols, { key: 'qty', dir: 'asc' })
  assert.deepEqual(asc.map(r => r.qty), ['2', '10', '30'])
  const desc = applySort(rows, cols, { key: 'qty', dir: 'desc' })
  assert.deepEqual(desc.map(r => r.qty), ['30', '10', '2'])
  const none = applySort(rows, cols, null)
  assert.deepEqual(none.map(r => r.id), ['1', '2', '3'])
})

test('nextSort cycles none→asc→desc→none for the active key', () => {
  assert.deepEqual(nextSort(null, 'qty'), { key: 'qty', dir: 'asc' })
  assert.deepEqual(nextSort({ key: 'qty', dir: 'asc' }, 'qty'), { key: 'qty', dir: 'desc' })
  assert.equal(nextSort({ key: 'qty', dir: 'desc' }, 'qty'), null)
  // switching to a different key starts ascending
  assert.deepEqual(nextSort({ key: 'qty', dir: 'desc' }, 'name'), { key: 'name', dir: 'asc' })
})

// TEST-4 — case-insensitive substring filter; empty passthrough; no-match empty
test('applyFilter keeps case-insensitive substring matches across columns', () => {
  assert.equal(matchesFilter(rows[1], cols, 'green'), true) // note "GREEN"
  assert.deepEqual(applyFilter(rows, cols, 'a').map(r => r.id), ['1', '2']) // Banana, apple
  assert.deepEqual(applyFilter(rows, cols, '   ').map(r => r.id), ['1', '2', '3']) // blank passthrough
  assert.deepEqual(applyFilter(rows, cols, 'zzz'), []) // no match
})

// TEST-5 — numeric detection: all-numeric only; empties ignored; mixed false; cap
test('detectNumericColumns marks a column numeric only when all sampled values are finite numbers', () => {
  const detected = detectNumericColumns(rows, cols)
  assert.equal(detected.has('qty'), true)
  assert.equal(detected.has('name'), false)
})

test('isNumericColumn ignores empties, rejects mixed, requires ≥1 value, and caps sampling', () => {
  const withEmpties: Row[] = [
    { id: '1', name: '', qty: '', note: '' },
    { id: '2', name: '', qty: '5', note: '' },
  ]
  assert.equal(isNumericColumn(withEmpties, { key: 'qty' }), true)
  assert.equal(isNumericColumn([{ id: '1', name: '', qty: '', note: '' }], { key: 'qty' }), false) // all empty
  const mixed: Row[] = [
    { id: '1', name: '', qty: '5', note: '' },
    { id: '2', name: '', qty: 'x', note: '' },
  ]
  assert.equal(isNumericColumn(mixed, { key: 'qty' }), false)
  // cap: a non-numeric beyond the cap is not sampled → still numeric
  const many: Row[] = Array.from({ length: 60 }, (_, i) => ({ id: String(i), name: '', qty: '1', note: '' }))
  many[55].qty = 'x'
  assert.equal(isNumericColumn(many, { key: 'qty' }, 50), true)
})

// TEST-6 — deriveView applies filter BEFORE sort
test('deriveView filters then sorts (filter narrows, then order applies)', () => {
  // query 'a' keeps Banana(qty10) + apple(qty2); sort qty desc → Banana, apple
  const out = deriveView(rows, cols, { sort: { key: 'qty', dir: 'desc' }, query: 'a' })
  assert.deepEqual(out.map(r => r.id), ['1', '2'])
})

// deriveView excludes a rowHeader gutter from filtering
test('deriveView filter ignores the rowHeader gutter column', () => {
  const withGutter: Row[] = [
    { id: '1', name: 'Banana', qty: '10', note: '' },
    { id: '2', name: 'apple', qty: '2', note: '' },
  ]
  const gutterCols: CoreColumn[] = [
    { key: '__rn', dataIndex: '__rn', rowHeader: true },
    { key: 'name', dataIndex: 'name' },
  ]
  const rowsWithRn = withGutter.map((r, i) => ({ ...r, __rn: String(i + 1) }))
  // Query "2" would match row #2's gutter number if the gutter were filtered;
  // it must instead match only real data (apple's qty is '2' but qty isn't in
  // these columns, so → no data match → empty).
  assert.deepEqual(deriveView(rowsWithRn, gutterCols, { sort: null, query: '2' }), [])
  // A real name match still works.
  assert.deepEqual(
    deriveView(rowsWithRn, gutterCols, { sort: null, query: 'apple' }).map(r => r.id),
    ['2'],
  )
})

// TEST-7 — selection serialisation: cell / row / multi-row range
test('serializeSelectionTsv renders cell, row, and multi-row selections', () => {
  const cell: TableSelection = { kind: 'cell', row: 0, col: 'name' }
  assert.equal(serializeSelectionTsv(cell, rows, cols), 'Banana')

  const oneRow: TableSelection = { kind: 'rows', rows: [1] }
  assert.equal(serializeSelectionTsv(oneRow, rows, cols), 'apple\t2\tGREEN')

  const range: TableSelection = { kind: 'rows', rows: [2, 0] } // out of order → sorted ascending
  assert.equal(
    serializeSelectionTsv(range, rows, cols),
    'Banana\t10\tripe\nCherry\t30\tred',
  )

  assert.equal(serializeSelectionTsv({ kind: 'none' }, rows, cols), '')
  // whole-view serialisation (used by "copy all")
  assert.equal(serializeTsv(rows, cols).split('\n').length, 3)
})

// clipboard formula-injection neutralization (opt-in sanitize)
test('serialize sanitize option neutralizes formula cells but keeps numbers', () => {
  const r: Row[] = [{ id: '1', name: '=SUM(A1)', qty: '-5', note: '@x' }]
  const raw = serializeSelectionTsv({ kind: 'rows', rows: [0] }, r, cols)
  assert.equal(raw, '=SUM(A1)\t-5\t@x') // unsanitized (default)
  const safe = serializeSelectionTsv({ kind: 'rows', rows: [0] }, r, cols, { sanitize: true })
  assert.equal(safe, "'=SUM(A1)\t-5\t'@x") // = and @ neutralized; -5 is a real number
  // single-cell selection sanitized too
  assert.equal(serializeSelectionTsv({ kind: 'cell', row: 0, col: 'name' }, r, cols, { sanitize: true }), "'=SUM(A1)")
})

// TEST-8 — width clamp never below minWidth (default 64), honours explicit min
test('clampWidth floors at the minimum width', () => {
  assert.equal(clampWidth(10), 64) // default floor
  assert.equal(clampWidth(200), 200)
  assert.equal(clampWidth(10, 120), 120) // explicit min
  assert.equal(clampWidth(150.6), 151) // rounded
})

// TEST-9 — last-visible guard
test('canHideColumn refuses to hide the last visible column', () => {
  assert.equal(canHideColumn(['a', 'b'], 'a'), true)
  assert.equal(canHideColumn(['a'], 'a'), false) // last one
  assert.equal(canHideColumn(['a', 'b'], 'zzz'), false) // not visible
})
