// @vitest-environment jsdom
//
// A `fill` tab panel must be a FLEX COLUMN, not a bounded block.
//
// The defect this pins: `fill` gave the active panel `flex-1 min-h-0 overflow-hidden` — the
// right height, the right clipping, and `display: block`. A block parent makes `flex-1` and
// `min-h-0` INERT on every child, so a panel hosting a flex-column pane (the shape every pane
// in this app has: a scrolling body plus something pinned beside it) laid that pane out at its
// natural content height and then clipped whatever did not fit. Clipped, not scrolled: the
// panel's own `overflow-hidden` swallowed it and no ancestor offered a track, so the content
// was UNREACHABLE rather than merely below the fold.
//
// Measured on a 390px phone before the fix: panel 688px, the pane inside it 896px, computed
// `overflow-y: hidden` — 208px gone, including the pane's primary action.
//
// jsdom has no layout engine and no Tailwind, so what is asserted here is the class contract
// on the panel element, which is the mechanism by which the layout happens. The two properties
// that matter are separable and both are checked: the panel is still BOUNDED (it must not go
// back to sizing to its content) and it is now a flex COLUMN (so a child's `flex-1 min-h-0`
// resolves). The end-to-end proof at real geometry is
// `src-app/ui/tests/e2e/14-declarative-qc/analysis-run-list-narrow.spec.ts`, which measures the
// live box chain at phone width; this file is what makes a regression cheap to catch.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { cleanup, render } from '@testing-library/react'
import { Tabs } from './tabs'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
})

afterEach(cleanup)

const items = [
  {
    key: 'a',
    label: 'A',
    // The shape that used to be clipped: a flex column that states its own height.
    children: <div data-testid="pane" className="flex min-h-0 flex-1 flex-col" />,
  },
  { key: 'b', label: 'B', children: <div /> },
]

function panel(fill: boolean): HTMLElement {
  render(<Tabs data-testid="tabs" fill={fill || undefined} items={items} />)
  const el = document.querySelector<HTMLElement>('[data-testid="tabs-panel-a"]')
  if (!el) throw new Error('the active panel did not render')
  return el
}

describe('Tabs fill: the panel gives its child a track', () => {
  it('is a flex COLUMN, so a `flex-1 min-h-0` child is not inert', () => {
    const el = panel(true)
    const cls = el.className.split(/\s+/)
    // Both halves. `flex` alone would lay the pane out in a ROW — a pane that is one column
    // wide and full height, which reads as "nothing rendered" rather than as a clipped pane.
    expect(cls, el.className).toContain('flex')
    expect(cls, el.className).toContain('flex-col')
  })

  it('is still BOUNDED — the fix must not trade clipping for an unbounded panel', () => {
    // The negative control for the change. Dropping `flex-1`/`min-h-0`/`overflow-hidden` would
    // also stop the content being clipped, by letting the panel grow past its container and
    // pushing the problem up to whatever encloses the Tabs. That is a different bug, not a fix.
    const cls = panel(true).className.split(/\s+/)
    expect(cls).toContain('flex-1')
    expect(cls).toContain('min-h-0')
    expect(cls).toContain('overflow-hidden')
  })

  it('leaves a NON-fill panel alone', () => {
    // `fill` is opt-in: a Tabs in normal document flow must keep sizing to its content, so the
    // panel carries none of the box classes above.
    const cls = panel(false).className.split(/\s+/)
    expect(cls).not.toContain('flex-col')
    expect(cls).not.toContain('overflow-hidden')
    expect(cls).not.toContain('min-h-0')
  })

  it('renders ONLY the active panel, so the flex display cannot reveal a hidden one', () => {
    // Why `flex` is safe to add here at all: base-ui's Tabs.Panel defaults `keepMounted={false}`,
    // so an inactive panel is absent from the DOM rather than present-and-`hidden`. Were it
    // present, an author-level `display: flex` would out-cascade the UA's `[hidden]{display:none}`
    // and every panel would draw at once. That is a property of the primitive underneath, which
    // is exactly the kind of thing a dependency bump changes silently.
    panel(true)
    expect(document.querySelector('[data-testid="tabs-panel-b"]')).toBeNull()
  })
})
