import * as React from 'react'
import {
  type CoreColumn,
  type SortState,
  type TableSelection,
  canHideColumn,
  clampWidth,
  deriveView,
  detectNumericColumns,
  nextSort,
  rowRange,
  serializeSelectionTsv,
  serializeTsv,
} from './table-view-core'

/** Options the hook needs from the Table props. Kept structural so `table.tsx`
 *  can pass its `TableColumn`s (a superset of `CoreColumn`) directly. */
export interface UseTableViewOptions<T> {
  columns: CoreColumn[]
  dataSource: T[]
  detectNumeric?: boolean
  defaultSort?: SortState | null
  defaultHidden?: string[]
}

export interface TableView<T> {
  // sort
  sort: SortState | null
  toggleSort: (key: string) => void
  // filter
  query: string
  setQuery: (q: string) => void
  // resize
  widths: Record<string, number>
  setWidth: (key: string, width: number) => void
  resetWidth: (key: string) => void
  // column visibility
  hidden: Set<string>
  toggleHidden: (key: string) => void
  isHidden: (key: string) => boolean
  // numeric detection
  numericKeys: Set<string>
  // selection
  selection: TableSelection
  selectCell: (row: number, col: string) => void
  selectRow: (row: number, mode: 'replace' | 'toggle' | 'range') => void
  clearSelection: () => void
  /** Serialise the current selection (or the whole view when empty) to TSV. */
  selectionText: (dataColumns?: CoreColumn[], sanitize?: boolean) => string
  // derived
  viewData: T[]
  visibleColumns: CoreColumn[]
}

/** Owns all interactive view state for the kit <Table> (DEC-1: uncontrolled).
 *  Pure derivation lives in `table-view-core`; this wires it to React state. */
export function useTableView<T>(opts: UseTableViewOptions<T>): TableView<T> {
  const { columns, dataSource } = opts
  const [sort, setSort] = React.useState<SortState | null>(opts.defaultSort ?? null)
  const [query, setQuery] = React.useState('')
  const [widths, setWidths] = React.useState<Record<string, number>>({})
  const [hidden, setHidden] = React.useState<Set<string>>(() => new Set(opts.defaultHidden ?? []))
  const [selection, setSelection] = React.useState<TableSelection>({ kind: 'none' })
  // Anchor for shift-range row selection (last single-selected row).
  const anchorRef = React.useRef<number | null>(null)

  const visibleColumns = React.useMemo(
    () => columns.filter(c => !hidden.has(c.key)),
    [columns, hidden],
  )

  const numericKeys = React.useMemo(
    () => (opts.detectNumeric ? detectNumericColumns(dataSource, columns) : keysOfExplicitNumeric(columns)),
    [opts.detectNumeric, dataSource, columns],
  )

  const viewData = React.useMemo(
    () => deriveView(dataSource, visibleColumns, { sort, query }),
    [dataSource, visibleColumns, sort, query],
  )

  // Selection indices are view-relative and reference a specific column, so
  // clear the selection whenever the view identity (sort/filter/dataSource) OR
  // the visible-column set changes — hiding a selected cell's column must not
  // leave an orphaned selection.
  React.useEffect(() => {
    setSelection({ kind: 'none' })
    anchorRef.current = null
  }, [viewData, visibleColumns])

  const toggleSort = React.useCallback((key: string) => {
    setSort(prev => nextSort(prev, key))
  }, [])

  const setWidth = React.useCallback((key: string, width: number) => {
    const col = columns.find(c => c.key === key)
    const min = (col as { minWidth?: number } | undefined)?.minWidth
    setWidths(prev => ({ ...prev, [key]: clampWidth(width, min) }))
  }, [columns])

  const resetWidth = React.useCallback((key: string) => {
    setWidths(prev => {
      if (!(key in prev)) return prev
      const next = { ...prev }
      delete next[key]
      return next
    })
  }, [])

  const toggleHidden = React.useCallback((key: string) => {
    setHidden(prev => {
      const currentlyVisible = columns.filter(c => !prev.has(c.key)).map(c => c.key)
      const next = new Set(prev)
      if (prev.has(key)) {
        next.delete(key)
      } else if (canHideColumn(currentlyVisible, key)) {
        next.add(key)
      }
      return next
    })
  }, [columns])

  const isHidden = React.useCallback((key: string) => hidden.has(key), [hidden])

  const selectCell = React.useCallback((row: number, col: string) => {
    anchorRef.current = row
    setSelection({ kind: 'cell', row, col })
  }, [])

  const selectRow = React.useCallback((row: number, mode: 'replace' | 'toggle' | 'range') => {
    // Anchor is updated OUTSIDE the state updater (a render-phase updater must be
    // side-effect-free; StrictMode double-invokes it).
    if (mode === 'range' && anchorRef.current != null) {
      const anchor = anchorRef.current
      setSelection({ kind: 'rows', rows: rowRange(anchor, row) })
      return
    }
    anchorRef.current = row
    if (mode === 'toggle') {
      setSelection(prev => {
        if (prev.kind === 'rows') {
          const has = prev.rows.includes(row)
          const rows = has ? prev.rows.filter(r => r !== row) : [...prev.rows, row]
          return rows.length === 0 ? { kind: 'none' } : { kind: 'rows', rows }
        }
        return { kind: 'rows', rows: [row] }
      })
      return
    }
    setSelection({ kind: 'rows', rows: [row] })
  }, [])

  const clearSelection = React.useCallback(() => {
    anchorRef.current = null
    setSelection({ kind: 'none' })
  }, [])

  const selectionText = React.useCallback(
    (dataColumns?: CoreColumn[], sanitize?: boolean) => {
      const cols = dataColumns ?? visibleColumns
      if (selection.kind === 'none' || (selection.kind === 'rows' && selection.rows.length === 0)) {
        return serializeTsv(viewData, cols, { sanitize })
      }
      return serializeSelectionTsv(selection, viewData, cols, { sanitize })
    },
    [selection, viewData, visibleColumns],
  )

  return {
    sort, toggleSort,
    query, setQuery,
    widths, setWidth, resetWidth,
    hidden, toggleHidden, isHidden,
    numericKeys,
    selection, selectCell, selectRow, clearSelection, selectionText,
    viewData, visibleColumns,
  }
}

function keysOfExplicitNumeric(columns: CoreColumn[]): Set<string> {
  const s = new Set<string>()
  for (const c of columns) if (c.numeric) s.add(c.key)
  return s
}
