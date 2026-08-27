// @vitest-environment jsdom
//
// A `Select` with NOTHING to offer must say so, in both arms.
//
// ## The defect
//
// `emptyText` existed and was documented, but only the SEARCHABLE arm consumed it. The base
// arm rendered `<SelectContent>{items}</SelectContent>` with `items === []`, so a caller with
// no options got a popup containing literally nothing. That is the one state a user cannot
// interpret: "this app has no models configured", "the list failed to load" and "my click did
// nothing" all look identical. The kit's own comment already drew the distinction between
// "nothing to choose from" (`-empty`) and "your filter matched nothing" (`-no-match`) — the
// first half had just never been implemented.
//
// The base arm is exactly the arm that gets used here: `showSearch: 'auto'` turns search on at
// five options, and a list with zero options is never searchable. So the blank popup was
// guaranteed for every empty Select in the app.
//
// ## The second half: the sentence has to be TRUE
//
// The searchable arm defaulted to "No match" whenever its list was empty, including when the
// user had typed nothing at all. "No match" is a claim about a query; with no query it is
// simply false, and it misdirects the reader into re-checking what they typed. An empty list
// with an empty query is the no-options state and now says so.
//
//   npx vitest run src/kit/select-empty-state.test.tsx

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

function mount(props: Partial<React.ComponentProps<typeof Select>> = {}): HTMLElement {
  render(
    <Select
      data-testid="picker"
      aria-label="Picker"
      options={[]}
      placeholder="Choose"
      {...(props as React.ComponentProps<typeof Select>)}
    />,
  )
  const el = document.querySelector<HTMLElement>('[data-testid="picker"]')
  if (!el) throw new Error('the picker did not render')
  return el
}

async function open(control: HTMLElement): Promise<void> {
  await act(async () => {
    fireEvent.click(control)
    await Promise.resolve()
  })
  await act(async () => {
    await Promise.resolve()
  })
}

const popup = (): HTMLElement | null =>
  document.querySelector<HTMLElement>('[data-testid="picker-popup"]')
const emptyRow = (): HTMLElement | null =>
  document.querySelector<HTMLElement>('[data-testid="picker-empty"]')
const noMatchRow = (): HTMLElement | null =>
  document.querySelector<HTMLElement>('[data-testid="picker-no-match"]')

describe('a Select with no options at all', () => {
  it('an empty list is BELOW the search threshold, so this is the base arm', () => {
    // Non-vacuity for the whole file: if zero options ever became searchable, every test here
    // would silently be exercising the other arm — the one that already worked.
    expect(0).toBeLessThan(SELECT_SEARCH_THRESHOLD)
  })

  it('says something rather than opening a blank popup', async () => {
    const el = mount()
    await open(el)

    const p = popup()
    expect(p, 'the popup must actually open — an absent assertion against a popup that never opened proves nothing').not.toBeNull()

    const row = emptyRow()
    expect(row, 'a Select with nothing to offer must render an empty state, not an empty box').not.toBeNull()
    expect(row?.textContent?.trim()).toBeTruthy()
  })

  it('the empty state is PROSE, not a selectable row', async () => {
    const el = mount()
    await open(el)

    // It must not be reachable as a choice: no option role, and nothing that typeahead or
    // arrow keys would land on. Otherwise the user can "select" a sentence.
    const row = emptyRow()
    expect(row?.getAttribute('role')).not.toBe('option')
    expect(
      document.querySelectorAll('[role="option"]').length,
      'an empty Select offers no options',
    ).toBe(0)
  })

  it("uses the caller's wording when given one", async () => {
    const el = mount({ emptyText: 'No models available' })
    await open(el)
    expect(emptyRow()?.textContent).toContain('No models available')
  })

  it('does NOT claim "No match" when the user has typed nothing', async () => {
    // The wording bug, pinned separately from the missing-element bug: a default of "No match"
    // is a statement about a query that does not exist, and sends the reader to check their
    // spelling instead of their configuration.
    const el = mount()
    await open(el)
    const row = emptyRow()
    // Non-vacuity: without this, a MISSING element satisfies the assertion below
    // (`undefined ?? ''` matches nothing), so the test would pass against the very
    // blank popup it exists to forbid. Verified: it does.
    expect(row, 'there must be an empty state to read before asking what it says').not.toBeNull()
    expect(row?.textContent ?? '').not.toMatch(/no match/i)
  })

  it('the no-options state is a DIFFERENT testid from the filter-matched-nothing state', async () => {
    // Callers query these by suffix and act differently on them. If one arm reused the other's
    // id, a test for "the list is empty" would pass on "your search found nothing".
    const el = mount()
    await open(el)
    expect(emptyRow()).not.toBeNull()
    expect(noMatchRow(), 'no query was typed, so this is not the no-match state').toBeNull()
  })
})

describe('a Select that still has options', () => {
  it('renders them, and no empty state', async () => {
    const el = mount({
      options: [
        { label: 'Option 1', value: 'o1' },
        { label: 'Option 2', value: 'o2' },
      ],
    })
    await open(el)

    // The control: the empty state must not leak into the ordinary case.
    expect(document.querySelectorAll('[role="option"]').length).toBe(2)
    expect(emptyRow(), 'a populated Select has no empty state').toBeNull()
  })
})
