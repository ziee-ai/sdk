import * as React from 'react'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

export interface DescriptionsItem {
  key: string
  label: React.ReactNode
  children: React.ReactNode
  span?: number
}

// legacy Descriptions: a label/value detail grid. Rendered as a semantic <table>
// (the way Ant Design does it) so cell content wraps to fit. On top of that it is
// RESPONSIVE like antd: the effective number of label/value pairs per row is
// reduced as the container narrows, so values keep enough width to wrap into
// words instead of being squeezed to one character per line. We measure the
// component's OWN container (ResizeObserver) rather than the viewport, so it
// adapts correctly inside a narrow drawer as well as a full-width page.
export type DescriptionsProps = {
  items: DescriptionsItem[]
  title?: React.ReactNode
  /** Max number of label/value pairs per row (default 1). Reduced automatically on narrow containers. */
  column?: number
  bordered?: boolean
  size?: 'sm' | 'default'
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
  className?: string} & KitStyleProps

// Minimum comfortable width (px) for one label+value pair before we drop a column.
const MIN_PAIR_WIDTH = 220

export function Descriptions({ items, title, column = 1, bordered, size = 'default', className, style, 'data-testid': testid }: DescriptionsProps) {
  const pad = size === 'sm' ? 'px-3 py-1.5 text-xs' : 'px-4 py-2 text-sm'
  const requested = Math.max(1, column)

  const rootRef = React.useRef<HTMLDivElement>(null)
  const [width, setWidth] = React.useState<number | null>(null)
  React.useLayoutEffect(() => {
    const el = rootRef.current
    if (!el) return
    setWidth(el.offsetWidth)
    const ro = new ResizeObserver((entries) => {
      setWidth(entries[0].contentRect.width)
    })
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  // How many pairs actually fit; clamp to the requested max. Until measured
  // (first paint), render the requested count.
  const cols =
    width == null ? requested : Math.max(1, Math.min(requested, Math.floor(width / MIN_PAIR_WIDTH)))

  // Chunk items into rows of at most `cols` "units" (an item with span S takes S
  // units), so multi-column layouts reflow into rows exactly like Ant Design.
  const rows: DescriptionsItem[][] = []
  let cur: DescriptionsItem[] = []
  let used = 0
  for (const it of items) {
    const span = Math.min(Math.max(it.span ?? 1, 1), cols)
    if (used + span > cols && cur.length) {
      rows.push(cur)
      cur = []
      used = 0
    }
    cur.push(it)
    used += span
    if (used >= cols) {
      rows.push(cur)
      cur = []
      used = 0
    }
  }
  if (cur.length) rows.push(cur)

  return (
    <div ref={rootRef} className={cn('w-full', className)} style={style} data-testid={testid}>
      {title != null && <div className="mb-2 font-semibold">{title}</div>}
      <div className={cn(bordered && 'overflow-hidden rounded-md border')}>
        <table className="w-full border-collapse text-left">
          <tbody>
            {rows.map((row, ri) => {
              const isLastRow = ri === rows.length - 1
              return (
                <tr key={ri}>
                  {row.map((it) => {
                    // A span-S value cell absorbs the (S-1) label columns it skips,
                    // matching the grid's `span * 2 - 1` behavior.
                    const valueColSpan = it.span && it.span > 1 ? it.span * 2 - 1 : 1
                    return (
                      <React.Fragment key={it.key}>
                        <th
                          scope="row"
                          className={cn(
                            pad,
                            'w-px whitespace-nowrap align-top font-medium text-muted-foreground',
                            bordered && cn('border-r bg-muted/40', !isLastRow && 'border-b'),
                          )}
                        >
                          {it.label}
                        </th>
                        <td
                          colSpan={valueColSpan}
                          className={cn(
                            pad,
                            'min-w-0 align-top [overflow-wrap:anywhere]',
                            bordered && cn('border-r last:border-r-0', !isLastRow && 'border-b'),
                          )}
                        >
                          {it.children}
                        </td>
                      </React.Fragment>
                    )
                  })}
                </tr>
              )
            })}
          </tbody>
        </table>
      </div>
    </div>
  )
}
