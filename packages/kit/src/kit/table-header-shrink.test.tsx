// @vitest-environment jsdom
//
// AN `ellipsis` COLUMN'S HEADING MUST GIVE WAY TOO — a column that is allowed to narrow cannot
// keep being floored by its own chrome.
//
// The defect: `ellipsis` put `truncate max-w-0` on the BODY cell only. A `<th>` is
// `whitespace-nowrap` (shadcn/table.tsx), so in an auto-layout table the column's minimum stayed
// the heading's FULL text — and `truncate` cannot change that, because `overflow:hidden` clips at
// the USED width and leaves min-content alone. Only a `max-width` caps a table column's minimum,
// which is exactly what the body cell already relied on.
//
// MEASURED, on the consumer's QC metrics form at its default pane width (300px region ⇒ 284px
// content box — `src-app/ui/tests/e2e/14-declarative-qc/metrics-table-error-geometry.spec.ts`):
//
//     grid 305px inside a 284px box, columns [60, 60, 138, 48]
//
// The 138 is the single word "Aggregation" (114px min-content) sitting over 88px of actual
// controls. The 21px of overflow is not paid by the heading — it is paid by the RIGHTMOST column,
// which is the destructive Remove button, so the cost of the floor is a destructive control the
// user has to scroll sideways to reach.
//
// WHY OPT-IN. Relaxing `min-width` on every heading would let EVERY table's headings collapse —
// a silent layout change across 18 kit-Table call sites in two repos, 9 of them in narrow
// containers, including file viewers whose whole job is a grid. `ellipsis` is the caller saying
// "this column may be abbreviated", so that is the only place the floor is lifted. The negative
// controls below are what keep that promise honest.
//
// jsdom has no layout engine, so what is asserted here is the CLASS CONTRACT — the mechanism by
// which the layout happens — exactly as `sheet-bottom-track.test.tsx` does. The geometry proof
// is the consumer's measured e2e above.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { Table, type TableColumn } from './table'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
})

afterEach(cleanup)

interface Row {
  id: string
  agg: string
}
const data: Row[] = [{ id: 'a', agg: 'mean' }]

function head(cols: TableColumn<Row>[], extra?: Partial<Record<string, unknown>>): HTMLElement[] {
  render(
    <Table<Row>
      data-testid="t"
      rowKey="id"
      columns={cols}
      dataSource={data}
      {...(extra as object)}
    />,
  )
  return Array.from(document.querySelectorAll<HTMLElement>('thead th'))
}

describe('an `ellipsis` column stops flooring the grid by its heading', () => {
  it('caps the header cell so the column can narrow below its heading', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation', ellipsis: true }])
    expect(
      th.className,
      'only a max-width caps a table column’s minimum — `truncate` alone does not',
    ).toContain('max-w-0')
  })

  it('lets the heading shrink inside its own flex row', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation', ellipsis: true }])
    const row = th.querySelector('span')!
    expect(
      row.className,
      'a flex item’s default min-width:auto is what hands min-content back up to the table',
    ).toContain('min-w-0')
  })

  it('keeps the full name readable as a native title', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation', ellipsis: true }])
    const label = th.querySelector('span.truncate')!
    expect(
      label.getAttribute('title'),
      'an abbreviated heading that cannot be read in full is a column with no name',
    ).toBe('Aggregation')
    expect(label.textContent, 'and the heading itself still carries the whole word').toBe('Aggregation')
  })

  it('does it for a SORTABLE heading too, where the label lives inside the sort button', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation', ellipsis: true, sortable: true }], {
      sortable: true,
    })
    const button = th.querySelector('button')!
    expect(button.className, 'the button is the flex item between the row and the label').toContain('min-w-0')
    expect(button.querySelector('span.truncate')!.getAttribute('title')).toBe('Aggregation')
    expect(th.getAttribute('aria-sort'), 'and it is still a sort control').toBe('none')
  })
})

describe('nothing else moves — the blast radius is zero by construction', () => {
  it('a column that did NOT ask to be narrowed keeps its floor', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation' }])
    expect(
      th.className,
      'lifting the floor for every table would silently reflow 18 call sites across two repos',
    ).not.toContain('max-w-0')
    expect(th.querySelector('span')!.className).not.toContain('min-w-0')
    expect(
      th.querySelector('span.truncate')!.hasAttribute('title'),
      'and it gains no title either — a title that repeats fully-visible text is noise',
    ).toBe(false)
  })

  it('an `ellipsis` column with an explicit width is left alone', () => {
    const [th] = head([{ key: 'agg', title: 'Aggregation', ellipsis: true, width: 200 }])
    expect(
      th.className,
      'max-width: 0 would fight the width the caller (or a resize drag) chose',
    ).not.toContain('max-w-0')
  })

  it('a non-string title gets no title attribute', () => {
    const [th] = head([{ key: 'agg', title: <em>Aggregation</em>, ellipsis: true }])
    expect(
      th.querySelector('span.truncate')!.hasAttribute('title'),
      'a ReactNode has no honest text form — "[object Object]" is not a label',
    ).toBe(false)
    expect(th.className, 'it still narrows, though — only the title is withheld').toContain('max-w-0')
  })

  it('the body cell keeps the truncation it always had', () => {
    head([{ key: 'agg', title: 'Aggregation', ellipsis: true }])
    const td = document.querySelector<HTMLElement>('tbody td')!
    expect(td.className).toContain('truncate')
    expect(td.className).toContain('max-w-0')
  })
})
