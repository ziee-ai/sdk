import * as React from 'react'
import { ArrowDown, ArrowUp, ChevronsUpDown, Columns3 } from 'lucide-react'
import { useVirtualizer } from '@tanstack/react-virtual'
import type { OverlayScrollbarsComponentRef } from 'overlayscrollbars-react'
import {
  Table as Base, TableHeader, TableBody, TableRow, TableHead, TableCell, TableCaption,
} from '../shadcn/table'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { ScrollArea } from './scroll-area'
import { Empty } from './empty'
import { Input } from './input'
import { Button } from './button'
import { Checkbox } from './checkbox'
import { Popover } from './popover'
import { useTableView, type TableView } from './use-table-view'
import { serializeSelectionTsv, type CoreColumn, type SortState } from './table-view-core'
import { cn } from '../lib/utils'

// legacy Table (common subset): columns + dataSource. Column.render gets (record, index).
export interface TableColumn<T> {
  key: string
  title: React.ReactNode
  /** Record field to read when `render` is omitted. Defaults to `key`. */
  dataIndex?: string
  render?: (record: T, index: number) => React.ReactNode
  align?: 'left' | 'center' | 'right'
  width?: number | string
  // ── opt-in capabilities (all backward-compatible; off unless the Table opts in) ──
  /** Allow sorting on this column (default: follows the Table's `sortable`). */
  sortable?: boolean
  /** Custom row comparator (asc); overrides the numeric-aware default. */
  sorter?: (a: T, b: T) => number
  /** Right-align + tabular-nums; also set automatically by `detectNumericColumns`. */
  numeric?: boolean
  /** Truncate the cell to one line + native `title` tooltip of the full value. */
  ellipsis?: boolean
  /** Can be toggled off in the column-chooser (default true when `columnChooser`). */
  hideable?: boolean
  /** Start hidden (column-chooser can re-show it). */
  defaultHidden?: boolean
  /** Allow resizing this column (default: follows the Table's `resizable`). */
  resizable?: boolean
  /** Minimum resized width in px (default 64). */
  minWidth?: number
  /** This column is a row selector: clicking a cell selects the whole ROW
   *  (used for the tabular viewer's `#` gutter). Excluded from copy/export. */
  rowHeader?: boolean
}

export interface TableProps<T> {
  columns: TableColumn<T>[]
  dataSource: T[]
  /** Row key: a record field name (legacy string form) or a function. */
  rowKey: (keyof T & string) | ((record: T, index: number) => string)
  /** Own loading → in-place skeleton rows. Region loading (surface) → skeleton too. */
  loading?: boolean
  caption?: React.ReactNode
  empty?: React.ReactNode
  className?: string
  onRowClick?: (record: T, index: number) => void
  /** Row-virtualize the body (only visible rows mount) for large data sets.
   *  Requires a bounded-height scroll ancestor (the kit ScrollArea / any
   *  overflow box); column `width`s are used for the flex row layout. */
  virtualized?: boolean
  /** Estimated row height (px) for the virtualizer; real heights are measured. */
  estimateRowHeight?: number
  /** Max height (CSS length) of the virtualized scroll box; short tables shrink
   *  to fit, taller ones cap here and scroll. Default `min(60vh, 36rem)`. */
  maxHeight?: string
  /** Fill the parent's height instead of hugging content: the root grows to
   *  `h-full` and the virtualized scroll box flexes to fill it (dropping the
   *  `maxHeight` cap) so the grid uses all available space and scrolls within
   *  it. Requires a definite-height parent + `virtualized`. Used by full-panel
   *  file viewers; inline previews leave it off so a 2-row table stays compact. */
  fillHeight?: boolean
  // ── opt-in Table-level capabilities ──
  /** Clickable sort on every column that doesn't set `sortable:false`. */
  sortable?: boolean
  /** Initial sort. */
  defaultSort?: SortState | null
  /** Render a toolbar search input; rows are filtered by case-insensitive
   *  substring across visible columns. */
  filterable?: boolean
  /** Placeholder for the filter input. */
  filterPlaceholder?: string
  /** Column-drag resize handles + a `<colgroup>`/fixed layout in the plain path. */
  resizable?: boolean
  /** Toolbar column-chooser (show/hide hideable columns). */
  columnChooser?: boolean
  /** Extra content rendered on the trailing (right) edge of the toolbar row —
   *  e.g. a row-count readout + jump-to-row control — so callers don't stack a
   *  second toolbar above this one. */
  toolbarExtra?: React.ReactNode
  /** Auto-detect all-numeric columns → right-align + tabular-nums. */
  detectNumericColumns?: boolean
  /** Cell/row selection + copy. `'cell'` selects single cells + rows (via a
   *  rowHeader column); `'row'` selects rows only. Ctrl/Cmd+C copies as TSV. */
  selectionMode?: 'none' | 'cell' | 'row'
  /** Called with the copied TSV after a selection copy. */
  onCopy?: (tsv: string) => void
  /** Neutralize spreadsheet formulas (=,+,-,@) in copied/serialised TSV so an
   *  untrusted cell can't execute when pasted into Excel/Sheets. */
  sanitizeClipboard?: boolean
  /** Called (from an effect) with the current selection serialised to TSV
   *  (empty string when nothing is selected) — lets an external "Copy" button
   *  copy the selection. */
  onSelectionChange?: (tsv: string) => void
  /** Called (from an effect) with the current filtered+sorted view whenever it
   *  changes, plus the currently-visible (non-gutter) column keys — for external
   *  readout / export / jump-to-row (export honours column-chooser visibility). */
  onViewChange?: (view: T[], meta: { visibleColumns: string[] }) => void
  /** View-relative index to scroll into view (virtual: scrollToIndex; plain:
   *  scrollIntoView). Change the value to trigger a scroll. */
  scrollToIndex?: number | null
  /**
   * PER-ROW DETAIL — a full-width band rendered as its own row directly BENEATH the row it
   * belongs to. Return `null`/`undefined` for a row that has no detail; that row renders
   * exactly as it does today.
   *
   * This is the one placement a columns-only grid cannot express. A value that belongs to a
   * row but needs the whole width — a range control over two of that row's own bounds, a
   * message, a preview — could previously only be squeezed into a column of its own, which
   * makes it narrow and puts it BESIDE the fields it is about rather than under them.
   *
   * The pair reads as ONE unit, which is the whole point of putting it under rather than
   * beside: the separator between them is suppressed so the rule falls after the BAND (i.e.
   * between records, never between a record and its own detail), the band follows its row's
   * hover, and DOM order is row → its band → next row, so keyboard order is too.
   *
   * NOT available on the virtualized path, and not silently so: the virtualizer measures ONE
   * `<tr>` per index and positions it absolutely, so a second row per index has nowhere to be
   * measured. Supplying this takes the plain path (see `showVirtual`), which is correct but
   * unwindowed — a caller with 100k rows should not want a detail band on each of them.
   */
  renderRowDetail?: (record: T, index: number) => React.ReactNode
  /** Test selector — forwarded onto <root>. Rows derive `${testid}-row-${rowKey}`. */
  'data-testid': string
}

const alignCls = { left: 'text-left', center: 'text-center', right: 'text-right' } as const
const justifyFor = { left: 'justify-start', center: 'justify-center', right: 'justify-end' } as const

// Default cell: render primitives directly; stringify anything else (a raw object/Date as a
// React child would throw). Columns needing rich content must supply `render`.
function defaultCell(v: unknown): React.ReactNode {
  if (v == null || typeof v === 'boolean') return null
  if (typeof v === 'object' && !React.isValidElement(v)) return String(v)
  return v as React.ReactNode
}

// Effective per-column presentation derived from the view state + Table flags.
interface ColMeta {
  align: 'left' | 'center' | 'right'
  numeric: boolean
  sortable: boolean
  resizable: boolean
  width: number | string | undefined
}
// Accessible label for a column: a string title verbatim, else the column key
// (never the `[object Object]` you'd get from stringifying a ReactNode title).
function colLabel<T>(col: TableColumn<T>): string {
  return typeof col.title === 'string' ? col.title : col.key
}

function colMeta<T>(col: TableColumn<T>, props: TableProps<T>, view: TableView<T>): ColMeta {
  const numeric = view.numericKeys.has(col.key)
  // explicit align wins; else numeric → right (routed through the UNCHANGED
  // alignCls/justifyFor maps so no new text-right literal enters the diff — DEC-6)
  const align: 'left' | 'center' | 'right' = col.align ?? (numeric ? 'right' : 'left')
  const sortable = !col.rowHeader && (col.sortable ?? !!props.sortable)
  const resizable = !!(props.resizable && (col.resizable ?? true))
  return { align, numeric, sortable, resizable, width: view.widths[col.key] ?? col.width }
}

// ── shared header inner (sort button) ─────────────────────────────────────────
function HeaderInner<T>({ col, meta, view, testid }: { col: TableColumn<T>; meta: ColMeta; view: TableView<T>; testid: string }) {
  const active = view.sort?.key === col.key
  const glyph = !active ? <ChevronsUpDown className="size-3.5 opacity-50" aria-hidden /> :
    view.sort!.dir === 'asc' ? <ArrowUp className="size-3.5" aria-hidden /> : <ArrowDown className="size-3.5" aria-hidden />
  return (
    <span className={cn('flex items-center gap-1', meta.numeric && 'flex-row-reverse')}>
      {meta.sortable ? (
        <button
          type="button"
          onClick={() => view.toggleSort(col.key)}
          // min-h-6 keeps the sort control on the 24px WCAG touch-target floor
          // (the bare text+glyph line is only ~20px tall).
          className="inline-flex min-h-6 items-center gap-1 -mx-1 px-1 rounded-sm hover:bg-muted/60 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
          data-testid={`${testid}-sort-${col.key}`}
        >
          <span className="truncate">{col.title}</span>
          {glyph}
        </button>
      ) : (
        <span className="truncate">{col.title}</span>
      )}
    </span>
  )
}

// Resize handle: a keyboard-focusable separator at the trailing edge of a header
// cell. Pointer drag updates the column width; double-click resets it.
function ResizeHandle<T>({ col, view, testid, width }: { col: TableColumn<T>; view: TableView<T>; testid: string; width?: number }) {
  const onPointerDown = (e: React.PointerEvent<HTMLSpanElement>) => {
    e.preventDefault()
    e.stopPropagation()
    const th = (e.currentTarget.closest('th') ?? e.currentTarget.parentElement) as HTMLElement | null
    // The drag must be tracked on the window this handle actually lives in — in a popped-out pane
    // that is NOT the ambient `window`, and listeners there would never fire. Mirrors
    // `@ziee/dock`'s useGripDrag, which resolves its window the same way.
    const win = e.currentTarget.ownerDocument?.defaultView ?? window
    const startX = e.clientX
    const startW = th ? th.getBoundingClientRect().width : 120
    const move = (ev: PointerEvent) => view.setWidth(col.key, startW + (ev.clientX - startX))
    const up = () => {
      win.removeEventListener('pointermove', move)
      win.removeEventListener('pointerup', up)
    }
    win.addEventListener('pointermove', move)
    win.addEventListener('pointerup', up)
  }
  const onKeyDown = (e: React.KeyboardEvent<HTMLSpanElement>) => {
    const th = (e.currentTarget.closest('th') ?? e.currentTarget.parentElement) as HTMLElement | null
    const cur = th ? th.getBoundingClientRect().width : 120
    if (e.key === 'ArrowLeft') { e.preventDefault(); view.setWidth(col.key, cur - 16) }
    else if (e.key === 'ArrowRight') { e.preventDefault(); view.setWidth(col.key, cur + 16) }
  }
  return (
    <span
      role="separator"
      aria-orientation="vertical"
      aria-label={`Resize column ${colLabel(col)}`}
      // A focusable separator MUST carry aria-valuenow (WAI-ARIA / axe
      // aria-required-attr). `width` is undefined until the column is measured,
      // so fall back to the min so the attr is always present (the drag/keyboard
      // handlers read the live width off the DOM, not this value).
      aria-valuenow={Math.round(width ?? col.minWidth ?? 64)}
      aria-valuemin={col.minWidth ?? 64}
      tabIndex={0}
      onPointerDown={onPointerDown}
      onKeyDown={onKeyDown}
      onDoubleClick={(e) => { e.stopPropagation(); view.resetWidth(col.key) }}
      data-testid={`${testid}-resize-${col.key}`}
      className="absolute inset-y-0 end-0 w-2 cursor-col-resize select-none touch-none hover:bg-border focus-visible:outline-none focus-visible:bg-ring/40"
    />
  )
}

// ── toolbar (search + column chooser) ─────────────────────────────────────────
function TableToolbar<T>({ props, view }: { props: TableProps<T>; view: TableView<T> }) {
  const testid = props['data-testid']
  const hideable = props.columnChooser
    ? props.columns.filter(c => !c.rowHeader && (c.hideable ?? true))
    : []
  // Count ALL currently-visible columns (incl. rowHeader/non-hideable) so the
  // last visible column's toggle is DISABLED (matches the hook's hide guard).
  const visibleCount = props.columns.filter(c => !view.isHidden(c.key)).length
  return (
    <div className="flex flex-wrap items-center gap-2 pb-2" data-testid={`${testid}-toolbar`}>
      {props.filterable && (
        <Input
          size="sm"
          allowClear
          // flex-1 (basis-0) so the filter shrinks to keep the Columns button on
          // the same row instead of wrapping below it; min-w-40 keeps the input
          // usable (never collapses to an unreadable sliver) and max-w-64 caps it
          // on a wide toolbar.
          className="flex-1 min-w-40 max-w-64"
          aria-label="Filter rows"
          placeholder={props.filterPlaceholder ?? 'Filter…'}
          value={view.query}
          onChange={e => view.setQuery(e.target.value)}
          data-testid={`${testid}-search`}
        />
      )}
      {props.columnChooser && hideable.length > 0 && (
        <Popover
          content={
            <div className="flex flex-col gap-1.5 p-1 min-w-40" data-testid={`${testid}-columns-menu`}>
              {hideable.map(c => (
                <Checkbox
                  key={c.key}
                  checked={!view.isHidden(c.key)}
                  disabled={!view.isHidden(c.key) && visibleCount <= 1}
                  onCheckedChange={() => view.toggleHidden(c.key)}
                  label={<span className="truncate">{c.title}</span>}
                  aria-label={`Toggle column ${colLabel(c)}`}
                  data-testid={`${testid}-col-toggle-${c.key}`}
                />
              ))}
            </div>
          }
        >
          <Button
            variant="outline"
            icon={<Columns3 />}
            aria-label="Choose columns"
            data-testid={`${testid}-columns-btn`}
          >
            Columns
          </Button>
        </Popover>
      )}
      {props.toolbarExtra && (
        <div className="ms-auto flex items-center gap-2">{props.toolbarExtra}</div>
      )}
    </div>
  )
}

export function Table<T>(props: TableProps<T>) {
  const s = useSurface({})
  const busy = !!(props.loading || s.loading)
  const view = useTableView<T>({
    columns: props.columns as unknown as CoreColumn[],
    dataSource: props.dataSource,
    detectNumeric: props.detectNumericColumns,
    defaultSort: props.defaultSort,
    defaultHidden: props.columns.filter(c => c.defaultHidden).map(c => c.key),
  })

  const selecting = (props.selectionMode ?? 'none') !== 'none'
  const sanitizeClipboard = !!props.sanitizeClipboard
  // Copy/export columns exclude any rowHeader gutter (DEC-8), and reflect the
  // CURRENT column-chooser visibility so hidden columns drop out of both.
  const copyColumns = React.useMemo(
    () => view.visibleColumns.filter(c => !(c as TableColumn<T>).rowHeader),
    [view.visibleColumns],
  )

  // Fire onViewChange from an effect (never during render) — ITEM-10. The
  // callback is held in a ref so an inline-arrow prop (the common case) does not
  // re-run the effect every render; it fires only when the view actually changes.
  // It reports the currently-visible (non-gutter) column keys so an external
  // export/copy excludes column-chooser-hidden columns.
  const onViewChangeRef = React.useRef(props.onViewChange)
  onViewChangeRef.current = props.onViewChange
  const visibleKeys = copyColumns.map(c => c.key)
  const visibleKeysSig = JSON.stringify(visibleKeys)
  React.useEffect(() => {
    onViewChangeRef.current?.(view.viewData, { visibleColumns: visibleKeys })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [view.viewData, visibleKeysSig])

  // Surface the current selection (empty when none) to an external Copy button.
  const onSelectionChangeRef = React.useRef(props.onSelectionChange)
  onSelectionChangeRef.current = props.onSelectionChange
  React.useEffect(() => {
    onSelectionChangeRef.current?.(serializeSelectionTsv(view.selection, view.viewData, copyColumns, { sanitize: sanitizeClipboard }))
  }, [view.selection, view.viewData, copyColumns, sanitizeClipboard])

  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    // Only handle copy when the table itself is focused (DEC-3: never hijack
    // page-wide copy). Ignore if the user is typing in the filter input.
    if ((e.key === 'c' || e.key === 'C') && (e.metaKey || e.ctrlKey)) {
      const target = e.target as HTMLElement
      if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return
      const tsv = view.selectionText(copyColumns, sanitizeClipboard)
      if (!tsv) return
      e.preventDefault()
      void navigator.clipboard?.writeText(tsv).then(() => props.onCopy?.(tsv)).catch(() => {})
    }
  }

  // Virtualize only when there's real data to window — loading/empty states use
  // the plain path (their skeleton/empty rows don't need a virtualizer).
  //
  // …and never when the caller renders a per-row detail band: the virtualizer measures ONE
  // `<tr>` per index and absolutely-positions it, so a second row per index has no slot to be
  // measured in and would overlap the next record. Falling back to the plain path renders the
  // detail CORRECTLY at the cost of windowing, which is the honest trade — dropping the band
  // instead would make a declared control silently absent.
  const showVirtual = props.virtualized && !busy && !props.renderRowDetail && view.viewData.length > 0
  const hasToolbar = !!(props.filterable || props.columnChooser || props.toolbarExtra)

  const body = showVirtual
    ? <VirtualTable {...props} view={view} />
    : <PlainTable {...props} view={view} busy={busy} />

  if (!hasToolbar && !selecting) return body
  return (
    <div
      className={cn('flex flex-col min-h-0', props.fillHeight && 'h-full')}
      onKeyDown={selecting ? onKeyDown : undefined}
      data-testid={`${props['data-testid']}-root`}
    >
      {hasToolbar && <TableToolbar props={props} view={view} />}
      {body}
    </div>
  )
}

// Compute the cell presentation (className + optional title) for a data cell.
function cellPresentation<T>(col: TableColumn<T>, meta: ColMeta, record: T): { className: string; title?: string } {
  const raw = (record as Record<string, unknown>)[col.dataIndex ?? col.key]
  const title = col.ellipsis && typeof raw === 'string' && raw !== '' ? raw : undefined
  return {
    className: cn(alignCls[meta.align], meta.numeric && 'tabular-nums', col.ellipsis && 'truncate max-w-0'),
    title,
  }
}

function PlainTable<T>(props: TableProps<T> & { view: TableView<T>; busy: boolean }) {
  const { rowKey, caption, empty, className, onRowClick, busy, fillHeight, view, 'data-testid': testid } = props
  const cols = view.visibleColumns as TableColumn<T>[]
  const rows = view.viewData
  const resizableTable = !!props.resizable
  // Column presentation computed ONCE per visible column (reused across header +
  // every body cell) rather than rows×cols times.
  const metas = new Map(cols.map(c => [c.key, colMeta(c, props, view)] as const))
  const rootRef = React.useRef<HTMLTableElement>(null)
  const keyOf = (record: T, i: number) =>
    typeof rowKey === 'function' ? rowKey(record, i) : String((record as Record<string, unknown>)[rowKey])

  // scrollToIndex (plain path): scroll the matching row into view.
  React.useEffect(() => {
    const idx = props.scrollToIndex
    if (idx == null || idx < 0 || idx >= rows.length) return
    const rk = keyOf(rows[idx], idx)
    rootRef.current
      ?.querySelector(`[data-testid="${testid}-row-${cssEscape(rk)}"]`)
      ?.scrollIntoView({ block: 'nearest' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.scrollToIndex])

  return (
    <Base
      ref={rootRef}
      className={cn(resizableTable && 'table-fixed', className)}
      // fillHeight: the table's own container flexes to fill the panel and
      // scrolls BOTH axes, so a small plain table keeps its content-auto column
      // widths (table-auto) instead of being forced onto the virtualized grid's
      // fixed widths.
      containerClassName={fillHeight ? 'flex-1 min-h-0 overflow-auto' : undefined}
      data-testid={testid}
    >
      {caption != null && <TableCaption>{caption}</TableCaption>}
      {resizableTable && (
        <colgroup>
          {cols.map(c => {
            const w = view.widths[c.key] ?? c.width
            return <col key={c.key} style={{ width: w }} />
          })}
        </colgroup>
      )}
      <TableHeader>
        <TableRow>
          {cols.map((c) => {
            const meta = metas.get(c.key)!
            return (
              <TableHead
                key={c.key}
                style={{ width: meta.width }}
                aria-sort={meta.sortable ? (view.sort?.key === c.key ? (view.sort.dir === 'asc' ? 'ascending' : 'descending') : 'none') : undefined}
                className={cn(alignCls[meta.align], meta.resizable && 'relative')}
              >
                <HeaderInner col={c} meta={meta} view={view} testid={testid} />
                {meta.resizable && <ResizeHandle col={c} view={view} testid={testid} width={typeof meta.width === 'number' ? meta.width : undefined} />}
              </TableHead>
            )
          })}
        </TableRow>
      </TableHeader>
      <TableBody>
        {busy ? (
          Array.from({ length: 3 }).map((_, r) => (
            <TableRow key={`sk-${r}`}>
              {cols.map((c) => (
                <TableCell key={c.key}><Skeleton className="h-4 w-full" /></TableCell>
              ))}
            </TableRow>
          ))
        ) : rows.length === 0 ? (
          <TableRow>
            <TableCell colSpan={cols.length} className="h-24">{empty ?? <Empty data-testid={`${testid}-empty`} />}</TableCell>
          </TableRow>
        ) : (
          rows.map((record, i) => {
            const rk = keyOf(record, i)
            // Evaluated ONCE per record: whether this row HAS a band decides the row's own
            // separator, so asking twice could disagree with itself.
            const detail = props.renderRowDetail?.(record, i)
            const banded = detail != null && detail !== false
            return (
              <React.Fragment key={rk}>
                <TableRow
                  data-testid={testid ? `${testid}-row-${rk}` : undefined}
                  tabIndex={onRowClick ? 0 : undefined}
                  onClick={onRowClick ? () => onRowClick(record, i) : undefined}
                  onKeyDown={
                    onRowClick
                      ? (e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault()
                            onRowClick(record, i)
                          }
                        }
                      : undefined
                  }
                  className={cn(
                    onRowClick && 'cursor-pointer focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50',
                    // A rule between a record and its OWN band would split the unit the band
                    // exists to be part of, so the rule falls after the band instead.
                    //
                    // Row-hover is suppressed on BOTH halves rather than made to follow: a
                    // highlight that covers only the top half splits the pair visually, and
                    // the CSS that would couple them (`peer-hover`) is the GENERAL sibling
                    // combinator — hovering row 0 would light every later band. A banded row
                    // is a form unit, not a selectable record; the separator is what says
                    // where it ends.
                    banded && 'border-b-0 hover:bg-transparent',
                  )}
                >
                  {cols.map((c) => {
                    const meta = metas.get(c.key)!
                    const pres = cellPresentation(c, meta, record)
                    const sel = cellSelected(props, view, i, c)
                    const selectable = selectionActive(props, c)
                    return (
                      <TableCell
                        key={c.key}
                        title={pres.title}
                        data-selected={sel || undefined}
                        // Focusable in selection mode so a keyboard user can select +
                        // the Ctrl/Cmd+C keydown bubbles to the wrapper's handler.
                        tabIndex={selectable ? 0 : undefined}
                        onClick={selectionHandler(props, view, i, c)}
                        className={cn(pres.className, sel && 'ring-2 ring-inset ring-ring/60', selectable && 'cursor-cell focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50')}
                      >
                        {c.render ? c.render(record, i) : defaultCell((record as Record<string, unknown>)[c.dataIndex ?? c.key])}
                      </TableCell>
                    )
                  })}
                </TableRow>
                {banded && (
                  <TableRow
                    data-testid={testid ? `${testid}-row-${rk}-detail` : undefined}
                    className="hover:bg-transparent"
                  >
                    {/* `whitespace-normal` undoes the data-cell default: a band is content,
                        not a value, and a value's no-wrap rule would force the table wider
                        than the viewport instead of letting the band use the width it has. */}
                    <TableCell colSpan={cols.length} className="pt-0 pb-3 align-top whitespace-normal">
                      {detail}
                    </TableCell>
                  </TableRow>
                )}
              </React.Fragment>
            )
          })
        )}
      </TableBody>
    </Base>
  )
}

// Row-virtualized table (TanStack `useVirtualizer`), used for large data grids.
function VirtualTable<T>(props: TableProps<T> & { view: TableView<T> }) {
  const {
    rowKey, className, onRowClick, estimateRowHeight = 40,
    maxHeight = 'min(60vh, 36rem)', fillHeight, view, 'data-testid': testid,
  } = props
  const cols = view.visibleColumns as TableColumn<T>[]
  const dataSource = view.viewData
  const metas = new Map(cols.map(c => [c.key, colMeta(c, props, view)] as const))
  const osRef = React.useRef<OverlayScrollbarsComponentRef>(null)
  const getScrollElement = React.useCallback(
    () => osRef.current?.osInstance()?.elements().viewport ?? null,
    [],
  )
  const [, setScrollReady] = React.useState(false)

  const keyOf = (record: T, i: number) =>
    typeof rowKey === 'function' ? rowKey(record, i) : String((record as Record<string, unknown>)[rowKey])

  const virt = useVirtualizer({
    count: dataSource.length,
    getScrollElement,
    estimateSize: () => estimateRowHeight,
    overscan: 12,
  })
  const items = virt.getVirtualItems()

  // scrollToIndex (virtual path).
  React.useEffect(() => {
    const idx = props.scrollToIndex
    if (idx != null && idx >= 0 && idx < dataSource.length) virt.scrollToIndex(idx, { align: 'center' })
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.scrollToIndex])

  const colStyle = (c: TableColumn<T>): React.CSSProperties => ({
    display: 'flex', width: view.widths[c.key] ?? c.width ?? 160, flexShrink: 0,
  })

  return (
    <ScrollArea
      ref={osRef}
      axis="both"
      autoHide="leave"
      events={{ initialized: () => setScrollReady(true) }}
      // fillHeight: flex to fill the parent (dropping the maxHeight cap) so the
      // grid uses all available panel height and scrolls within it.
      style={fillHeight ? undefined : { maxHeight }}
      className={cn(
        'w-full rounded-md border border-border bg-background',
        fillHeight && 'flex-1 min-h-0',
      )}
    >
      <table data-testid={testid} className={cn('w-max text-sm', className)} style={{ display: 'grid' }}>
        <thead className="sticky top-0 z-[1] bg-muted/80" style={{ display: 'grid' }}>
          <tr className="border-b border-border" style={{ display: 'flex', width: '100%' }}>
            {cols.map((c) => {
              const meta = metas.get(c.key)!
              return (
                <th
                  key={c.key}
                  style={colStyle(c)}
                  aria-sort={meta.sortable ? (view.sort?.key === c.key ? (view.sort.dir === 'asc' ? 'ascending' : 'descending') : 'none') : undefined}
                  className={cn('px-4 py-2 font-semibold', justifyFor[meta.align], meta.resizable && 'relative')}
                >
                  <HeaderInner col={c} meta={meta} view={view} testid={testid} />
                  {meta.resizable && <ResizeHandle col={c} view={view} testid={testid} width={typeof meta.width === 'number' ? meta.width : undefined} />}
                </th>
              )
            })}
          </tr>
        </thead>
        <tbody style={{ display: 'grid', height: virt.getTotalSize(), position: 'relative' }}>
          {items.map((vi) => {
            const record = dataSource[vi.index]
            return (
              <tr
                key={keyOf(record, vi.index)}
                data-index={vi.index}
                ref={virt.measureElement}
                data-testid={testid ? `${testid}-row-${keyOf(record, vi.index)}` : undefined}
                onClick={onRowClick ? () => onRowClick(record, vi.index) : undefined}
                className={cn('border-b border-border', onRowClick && 'cursor-pointer hover:bg-muted/50')}
                style={{ display: 'flex', position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${vi.start}px)` }}
              >
                {cols.map((c) => {
                  const meta = metas.get(c.key)!
                  const pres = cellPresentation(c, meta, record)
                  const sel = cellSelected(props, view, vi.index, c)
                  const selectable = selectionActive(props, c)
                  return (
                    <td
                      key={c.key}
                      title={pres.title}
                      data-selected={sel || undefined}
                      tabIndex={selectable ? 0 : undefined}
                      onClick={selectionHandler(props, view, vi.index, c)}
                      style={{ ...colStyle(c), minWidth: 0 }}
                      className={cn('px-4 py-2', justifyFor[meta.align], meta.numeric && 'tabular-nums', c.ellipsis && 'truncate', sel && 'ring-2 ring-inset ring-ring/60', selectable && 'cursor-cell focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50')}
                    >
                      {c.render ? c.render(record, vi.index) : defaultCell((record as Record<string, unknown>)[c.dataIndex ?? c.key])}
                    </td>
                  )
                })}
              </tr>
            )
          })}
        </tbody>
      </table>
    </ScrollArea>
  )
}

// ── selection helpers (shared by both paths) ──────────────────────────────────
function selectionActive<T>(props: TableProps<T>, col: TableColumn<T>): boolean {
  const mode = props.selectionMode ?? 'none'
  if (mode === 'none') return false
  if (col.rowHeader) return true
  return mode === 'cell'
}
function cellSelected<T>(props: TableProps<T>, view: TableView<T>, rowIdx: number, col: TableColumn<T>): boolean {
  if ((props.selectionMode ?? 'none') === 'none') return false
  const sel = view.selection
  if (sel.kind === 'cell') return sel.row === rowIdx && sel.col === col.key && !col.rowHeader
  if (sel.kind === 'rows') return sel.rows.includes(rowIdx)
  return false
}
function selectionHandler<T>(props: TableProps<T>, view: TableView<T>, rowIdx: number, col: TableColumn<T>) {
  if (!selectionActive(props, col)) return undefined
  return (e: React.MouseEvent) => {
    e.stopPropagation()
    if (col.rowHeader || (props.selectionMode ?? 'none') === 'row') {
      const mode = e.shiftKey ? 'range' : (e.metaKey || e.ctrlKey) ? 'toggle' : 'replace'
      view.selectRow(rowIdx, mode)
    } else {
      view.selectCell(rowIdx, col.key)
    }
  }
}

// Minimal CSS.escape shim for the plain-path row lookup (older engines).
function cssEscape(s: string): string {
  const g = globalThis as { CSS?: { escape?: (v: string) => string } }
  if (g.CSS?.escape) return g.CSS.escape(s)
  return s.replace(/["\\\]]/g, '\\$&')
}
