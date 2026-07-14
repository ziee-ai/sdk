// Pure, React-free view derivation + serialisation for the kit <Table>.
//
// Kept dependency-free and erasable-TS (interfaces + plain functions, no enums /
// namespaces / decorators) so `node --test "src/**/*.test.ts"` can import and
// exercise it with Node's native type-stripping — no bundler, no DOM. The React
// hook (`use-table-view.ts`) and the render paths (`table.tsx`) wrap these.

export type SortDir = 'asc' | 'desc'
export interface SortState {
  key: string
  dir: SortDir
}

/** Minimal column shape the core needs — a structural subset of `TableColumn`. */
export interface CoreColumn {
  key: string
  /** Field read from the record when no custom accessor is given. Defaults to `key`. */
  dataIndex?: string
  /** Explicitly mark as a numeric column (right-align + tabular-nums). */
  numeric?: boolean
  /** Custom comparator over two records (overrides the default). */
  sorter?: (a: unknown, b: unknown) => number
  /** Custom text used for filtering (defaults to the stringified cell value). */
  filterText?: (record: unknown) => string
  /** Row-selector gutter column — excluded from filtering (as it is from
   *  copy/export/column-chooser/numeric detection). */
  rowHeader?: boolean
}

/** Selection model (see DEC-8): none, a single cell, or a set of whole rows.
 *  Row indices are VIEW-relative (indices into the derived view). */
export type TableSelection =
  | { kind: 'none' }
  | { kind: 'cell'; row: number; col: string }
  | { kind: 'rows'; rows: number[] }

const NONE: TableSelection = { kind: 'none' }
export const EMPTY_SELECTION = NONE

/** Read the underlying data value for a column from a record. */
export function getCellValue<T>(record: T, col: CoreColumn): unknown {
  return (record as Record<string, unknown>)[col.dataIndex ?? col.key]
}

/** Stringify a cell value for filtering / comparison / copy (never throws). */
export function cellString(v: unknown): string {
  if (v == null) return ''
  if (typeof v === 'boolean') return v ? 'true' : 'false'
  return String(v)
}

/** Parse a value as a finite number, or NaN. Blank strings are NOT numbers. */
function asNumber(v: unknown): number {
  if (typeof v === 'number') return Number.isFinite(v) ? v : NaN
  const s = cellString(v).trim()
  if (s === '') return NaN
  // Tolerate thousands separators + a single leading currency-ish symbol so a
  // column like "1,234" / "$12.50" still reads as numeric.
  const cleaned = s.replace(/^[$€£¥]/, '').replace(/,/g, '')
  if (cleaned === '' || !/^[-+]?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?%?$/.test(cleaned)) return NaN
  return parseFloat(cleaned)
}

/** True when the value parses as a finite number (blank = not numeric). */
export function isNumericValue(v: unknown): boolean {
  return !Number.isNaN(asNumber(v))
}

/** Compare two cell values: numerically when both are numbers, else by locale
 *  string compare. Stable, total ordering. */
export function compareValues(a: unknown, b: unknown): number {
  const na = asNumber(a)
  const nb = asNumber(b)
  const aNum = !Number.isNaN(na)
  const bNum = !Number.isNaN(nb)
  if (aNum && bNum) return na - nb
  // A numeric cell sorts before a non-numeric one for a mixed column (stable).
  if (aNum !== bNum) return aNum ? -1 : 1
  return cellString(a).localeCompare(cellString(b), undefined, { numeric: true, sensitivity: 'base' })
}

function comparatorFor<T>(col: CoreColumn): (a: T, b: T) => number {
  if (col.sorter) return col.sorter as (a: T, b: T) => number
  return (a, b) => compareValues(getCellValue(a, col), getCellValue(b, col))
}

/** Apply a tri-state sort. `sort === null` returns the input order unchanged.
 *  Stable (index-tagged) so equal rows keep their relative order, and `none`
 *  (a null sort) restores the original dataSource order exactly. */
export function applySort<T>(rows: T[], columns: CoreColumn[], sort: SortState | null): T[] {
  if (!sort) return rows
  const col = columns.find(c => c.key === sort.key)
  if (!col) return rows
  const cmp = comparatorFor<T>(col)
  const dir = sort.dir === 'desc' ? -1 : 1
  return rows
    .map((r, i) => [r, i] as const)
    .sort((x, y) => {
      const c = cmp(x[0], y[0])
      return c !== 0 ? c * dir : x[1] - y[1]
    })
    .map(pair => pair[0])
}

/** Text of a record for a single column, honouring a custom `filterText`. */
function rowColText<T>(record: T, col: CoreColumn): string {
  return col.filterText ? col.filterText(record) : cellString(getCellValue(record, col))
}

/** True when any of the given (visible) columns' cell text contains `query`
 *  case-insensitively. An empty/blank query matches everything. */
export function matchesFilter<T>(record: T, columns: CoreColumn[], query: string): boolean {
  const q = query.trim().toLowerCase()
  if (q === '') return true
  for (const col of columns) {
    if (rowColText(record, col).toLowerCase().includes(q)) return true
  }
  return false
}

/** Keep rows matching the query across the given columns. Empty query = passthrough. */
export function applyFilter<T>(rows: T[], columns: CoreColumn[], query: string): T[] {
  if (query.trim() === '') return rows
  return rows.filter(r => matchesFilter(r, columns, query))
}

/** Derive the view: filter FIRST, then sort (DEC-6/9). Filtering runs over the
 *  visible NON-gutter columns (a rowHeader gutter's row numbers must not match a
 *  numeric query), while sort resolves its key against all visible columns. */
export function deriveView<T>(
  rows: T[],
  visibleColumns: CoreColumn[],
  opts: { sort: SortState | null; query: string },
): T[] {
  const filterColumns = visibleColumns.filter(c => !c.rowHeader)
  return applySort(applyFilter(rows, filterColumns, opts.query), visibleColumns, opts.sort)
}

export const NUMERIC_SAMPLE_CAP = 50

/** True when EVERY sampled non-empty value in the column parses as a finite
 *  number. Empty cells are ignored; an all-empty column is NOT numeric.
 *  Sampling caps at `NUMERIC_SAMPLE_CAP` rows for large data sets. */
export function isNumericColumn<T>(rows: T[], col: CoreColumn, cap = NUMERIC_SAMPLE_CAP): boolean {
  let seen = 0
  for (let i = 0; i < rows.length && seen < cap; i++) {
    const v = getCellValue(rows[i], col)
    if (cellString(v).trim() === '') continue
    seen++
    if (!isNumericValue(v)) return false
  }
  return seen > 0
}

/** Keys of the columns detected as numeric (used when `detectNumericColumns`). */
export function detectNumericColumns<T>(rows: T[], columns: CoreColumn[]): Set<string> {
  const keys = new Set<string>()
  for (const col of columns) {
    if (col.numeric || isNumericColumn(rows, col)) keys.add(col.key)
  }
  return keys
}

export const DEFAULT_MIN_WIDTH = 64

/** Clamp a resized width to at least `minWidth` (default 64px). */
export function clampWidth(width: number, minWidth: number = DEFAULT_MIN_WIDTH): number {
  const floor = Math.max(1, minWidth)
  return Math.max(floor, Math.round(width))
}

/** Last-visible guard: a column may be hidden only if ≥1 column stays visible. */
export function canHideColumn(currentlyVisibleKeys: string[], key: string): boolean {
  return currentlyVisibleKeys.includes(key) && currentlyVisibleKeys.length > 1
}

/** Options for TSV serialisers. `sanitize` neutralizes spreadsheet formulas so
 *  a copied/exported cell can't execute when pasted into Excel/Sheets. */
export interface TsvOptions {
  sanitize?: boolean
}

/** True when a string parses as a plain finite number (so a legit negative like
 *  "-5" is not treated as a formula). */
function isPlainNumber(v: string): boolean {
  const s = v.trim()
  return s !== '' && Number.isFinite(Number(s))
}

/** Formula/CSV-injection neutralization: a spreadsheet evaluates a cell that
 *  begins with = + - @ (or a leading tab/CR) as a formula. Prefix such a cell
 *  (that is not a real number) with a single quote so it stays literal text. */
export function neutralizeSpreadsheetCell(v: string): string {
  if (v === '') return v
  if (/^[=+\-@\t\r]/.test(v) && !isPlainNumber(v)) return `'${v}`
  return v
}

function fieldOf(v: unknown, sanitize: boolean): string {
  const s = cellString(v)
  return sanitize ? neutralizeSpreadsheetCell(s) : s
}

/** A row's visible cells as a tab-joined line. */
function rowToTsvLine<T>(record: T, columns: CoreColumn[], sanitize: boolean): string {
  return columns.map(c => fieldOf(getCellValue(record, c), sanitize)).join('\t')
}

/** Serialise the whole view (no header) to newline-joined TSV. */
export function serializeTsv<T>(rows: T[], columns: CoreColumn[], opts: TsvOptions = {}): string {
  return rows.map(r => rowToTsvLine(r, columns, !!opts.sanitize)).join('\n')
}

/** Serialise the current selection to TSV (DEC-8):
 *  - cell → the single value,
 *  - rows → each selected row's visible cells (tab-joined), rows newline-joined
 *    in ascending view order,
 *  - none → ''.
 *  `columns` are the data columns to include (caller excludes any gutter). */
export function serializeSelectionTsv<T>(
  selection: TableSelection,
  viewRows: T[],
  columns: CoreColumn[],
  opts: TsvOptions = {},
): string {
  const sanitize = !!opts.sanitize
  if (selection.kind === 'none') return ''
  if (selection.kind === 'cell') {
    const record = viewRows[selection.row]
    if (record == null) return ''
    const col = columns.find(c => c.key === selection.col)
    if (!col) return ''
    return fieldOf(getCellValue(record, col), sanitize)
  }
  const ordered = [...selection.rows].sort((a, b) => a - b)
  return ordered
    .map(i => viewRows[i])
    .filter((r): r is T => r != null)
    .map(r => rowToTsvLine(r, columns, sanitize))
    .join('\n')
}

/** True when the selection contains no cells/rows. */
export function isEmptySelection(sel: TableSelection): boolean {
  return sel.kind === 'none' || (sel.kind === 'rows' && sel.rows.length === 0)
}

/** Toggle a tri-state sort for a header click: a new key starts ascending;
 *  the active key cycles asc → desc → none (null). */
export function nextSort(current: SortState | null, key: string): SortState | null {
  if (!current || current.key !== key) return { key, dir: 'asc' }
  if (current.dir === 'asc') return { key, dir: 'desc' }
  return null
}

/** Extend a row selection to an inclusive range [anchor..target] (view indices). */
export function rowRange(anchor: number, target: number): number[] {
  const lo = Math.min(anchor, target)
  const hi = Math.max(anchor, target)
  const out: number[] = []
  for (let i = lo; i <= hi; i++) out.push(i)
  return out
}
