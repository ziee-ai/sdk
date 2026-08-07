// @vitest-environment jsdom
//
// The ONE property of `Table`'s per-row detail band that no consuming app can assert: what
// happens when a caller asks for a band AND for virtualization.
//
// Everything else about the band — that it renders under its own record, spans every column,
// carries the row's index, and suppresses the separator between a record and its own band — is
// asserted where the band is actually used, through the real declarative renderer
// (`src-app/ui/.../render/tableRowControls.test.tsx` in the consuming app). This file exists for
// the branch that has no consumer: the app's declarative table never sets `virtualized`, so the
// interaction between the two props would otherwise be reasoned about and never run.
//
// It matters because the failure is not a visual glitch. The virtualizer measures ONE `<tr>` per
// index and positions each absolutely from that measurement; a second `<tr>` per index is not
// measured, so it would be drawn over the following record. The prop therefore drops the table
// onto the plain path — correct, unwindowed — and the two assertions below are the two halves of
// that being true rather than intended: the band IS rendered, and the virtual path was NOT taken.

import { describe, it, expect, afterEach } from 'vitest'
import * as React from 'react'
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { Table } from './table'

interface Rec { id: string; name: string }
const DATA: Rec[] = [
  { id: 'a', name: 'first' },
  { id: 'b', name: 'second' },
  { id: 'c', name: 'third' },
]

let root: Root | null = null
let host: HTMLDivElement | null = null

function mount(node: React.ReactElement): HTMLElement {
  host = document.createElement('div')
  document.body.appendChild(host)
  root = createRoot(host)
  act(() => root!.render(node))
  return host
}

afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  root = null
  host = null
})

const table = (over: Partial<React.ComponentProps<typeof Table<Rec>>> = {}) => (
  <Table<Rec>
    data-testid="t"
    rowKey="id"
    dataSource={DATA}
    columns={[
      { key: 'name', title: 'Name' },
      { key: 'id', title: 'Id' },
    ]}
    {...over}
  />
)

describe('Table — a per-row detail band and virtualization cannot both be honoured', () => {
  it('renders the band, and does NOT take the virtual path, when both are asked for', () => {
    const el = mount(table({ virtualized: true, renderRowDetail: r => <span>detail-{r.id}</span> }))
    // The band IS there — the request was honoured, not dropped. A dropped band is the failure
    // this branch exists to avoid: a declared control silently absent reads as a bug in the
    // caller's own spec.
    expect(el.querySelector('[data-testid="t-row-b-detail"]')).not.toBeNull()
    expect(el.textContent).toContain('detail-b')
    // …and the PLAIN path drew it. The virtual path is identified by what only it emits: an
    // absolutely-positioned body with a computed total height. Asserting the absence of that,
    // rather than the presence of a `<tbody>`, is what makes this fail if the exclusion is
    // dropped — both paths render a tbody.
    const tbody = el.querySelector('tbody') as HTMLElement
    expect(tbody.style.position, 'the virtualized body was used').toBe('')
    expect(tbody.style.height).toBe('')
  })

  it('still virtualizes when NO band is asked for — the negative control', () => {
    // Without this half, an implementation that simply never virtualizes would satisfy the
    // assertions above while quietly costing every large grid its windowing.
    const el = mount(table({ virtualized: true }))
    const tbody = el.querySelector('tbody') as HTMLElement
    expect(tbody.style.position, 'the virtual path was not taken at all').toBe('relative')
  })

  it('a row whose detail is null gets no band row — one band per record, not one per row set', () => {
    const el = mount(table({ renderRowDetail: r => (r.id === 'b' ? <span>only-b</span> : null) }))
    expect(el.querySelector('[data-testid="t-row-b-detail"]')).not.toBeNull()
    expect(el.querySelector('[data-testid="t-row-a-detail"]')).toBeNull()
    expect(el.querySelector('[data-testid="t-row-c-detail"]')).toBeNull()
  })
})
