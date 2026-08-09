// @vitest-environment jsdom
//
// WHERE `showSearch: 'auto'` flips — pinned to the LITERAL count, in the kit that owns it.
//
// The consuming app already asserts both sides of this boundary
// (`workbench/extensions/widgets/renderers.search.test.tsx`), but it does so SYMBOLICALLY:
// it mounts `SELECT_SEARCH_THRESHOLD - 1` options and then `SELECT_SEARCH_THRESHOLD`. Those
// tests re-derive the boundary from the number under test, so they follow the constant
// wherever it goes — set it to 10, to 5, or to 400 and they stay green. They prove the
// MECHANISM ('auto' has two sides) and say nothing whatever about the VALUE.
//
// That is the exact hole a threshold change falls through, so this file closes it by writing
// the numbers down: four options is a list you read, five is a list you search. A future
// change to the constant now has to come here and say so, which is the point — the previous
// value (10) was chosen for a reason that is recorded in `select.tsx`, was later judged wrong,
// and moved without a single test noticing.
//
// Both sides are asserted, and the negative one is not vacuous: it first checks the popup is
// really OPEN (the options are on screen) and only then that no search field is among them. An
// "absent" assertion against a popup that never opened passes having looked at nothing.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { Select, SELECT_SEARCH_THRESHOLD } from './select'

// ── jsdom gaps the kit's base-ui controls probe at mount ─────────────────────────────────
beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
  if (!globalThis.CSS) (globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v }
  else if (typeof globalThis.CSS.escape !== 'function') {
    ;(globalThis.CSS as { escape: (v: string) => string }).escape = (v: string) => v
  }
  if (typeof (globalThis as { PointerEvent?: unknown }).PointerEvent !== 'function') {
    ;(globalThis as { PointerEvent?: unknown }).PointerEvent = class extends MouseEvent {} as never
    ;(window as unknown as { PointerEvent?: unknown }).PointerEvent = (
      globalThis as { PointerEvent?: unknown }
    ).PointerEvent
  }
  if (!window.matchMedia) {
    window.matchMedia = ((q: string) => ({
      matches: false,
      media: q,
      addEventListener() {},
      removeEventListener() {},
      addListener() {},
      removeListener() {},
      dispatchEvent: () => false,
      onchange: null,
    })) as never
  }
})

afterEach(cleanup)

/** `n` plain options — no groups, no disabled rows, nothing but a count. */
const options = (n: number) =>
  Array.from({ length: n }, (_, i) => ({ label: `Option ${i + 1}`, value: `o${i + 1}` }))

function mount(n: number, showSearch?: boolean | 'auto'): HTMLElement {
  render(
    <Select
      data-testid="picker"
      aria-label="Picker"
      options={options(n)}
      showSearch={showSearch}
      placeholder="Choose"
    />,
  )
  const el = document.querySelector<HTMLElement>('[data-testid="picker"]')
  if (!el) throw new Error('the picker did not render')
  return el
}

/** Open the popup the way a user does. Both arms open on a click of the trigger. */
async function open(control: HTMLElement): Promise<void> {
  await act(async () => {
    fireEvent.click(control)
    await Promise.resolve()
  })
  await act(async () => {
    await Promise.resolve()
  })
}

const searchField = (): HTMLElement | null =>
  document.querySelector<HTMLElement>('[data-testid="picker-search"]')

const offeredRows = (): HTMLElement[] => [
  ...document.querySelectorAll<HTMLElement>('[role="option"][data-testid^="picker-opt-"]'),
]

describe('the `auto` search threshold, at its literal value', () => {
  it('is FIVE', () => {
    // Written out, not computed. This is the assertion that makes the two boundary tests below
    // mean something: without it they are satisfied by any threshold at all, since each mounts
    // a count and asks what the control did with it.
    expect(SELECT_SEARCH_THRESHOLD).toBe(5)
  })

  it('a list of FOUR is offered whole, with no search field', async () => {
    const el = mount(4)
    await open(el)
    // Non-vacuity first: the popup is open and all four rows are on screen. If this ever
    // reports zero, the assertion underneath is measuring a closed popup, not a design choice.
    expect(offeredRows().map(r => r.getAttribute('data-testid'))).toEqual([
      'picker-opt-o1',
      'picker-opt-o2',
      'picker-opt-o3',
      'picker-opt-o4',
    ])
    expect(searchField(), 'four options must not come with a search box').toBeNull()
  })

  it('a list of FIVE gets one', async () => {
    const el = mount(5)
    await open(el)
    expect(offeredRows()).toHaveLength(5)
    expect(searchField(), 'five options must be searchable').not.toBeNull()
  })
})

describe('`auto` stays a DEFAULT, not a mandate', () => {
  // The threshold moving down is a nudge, not a removal of the caller's say. A change that
  // made every Select searchable (or that hard-wired the count) would pass both boundary tests
  // above if it also happened to keep 'auto' — these two are what tell those cases apart.
  it('`showSearch={false}` keeps a long list unsearchable', async () => {
    const el = mount(12, false)
    await open(el)
    expect(offeredRows()).toHaveLength(12)
    expect(searchField()).toBeNull()
  })

  it('`showSearch` keeps a short list searchable', async () => {
    const el = mount(2, true)
    await open(el)
    expect(searchField()).not.toBeNull()
  })
})
