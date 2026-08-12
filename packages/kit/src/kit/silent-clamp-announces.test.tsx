// @vitest-environment jsdom
//
// A CONTROL THAT REWRITES WHAT THE USER SUPPLIED SAYS SO.
//
// ── Why this is the same disease as the refusal work, one step earlier ──────────────────────
// A refusal that explains itself is now the house standard. A SILENT REWRITE is strictly worse:
// the user is not merely refused without a reason, they are not told they were refused at all.
// The control accepts the interaction, changes the value, and shows a result that looks like
// what they asked for.
//
// ── THE RULE (stated once, here, because the next author needs it) ──────────────────────────
//   1. NORMALIZATION — the value means the same thing, spelled differently: `"1."`→`1`,
//      `" x "`→`"x"`, a search query case-folded. STAYS SILENT. Announcing it would be noise,
//      and the user cannot perceive a difference to be confused by.
//   2. CLAMP / ROUND / REVERT — the value the control keeps is one the user can perceive as
//      DIFFERENT from the one they entered. MUST ANNOUNCE, at the moment it happens, in a live
//      region so it reaches assistive tech too. It is not refused — the interaction succeeds —
//      so an error state would be a lie; it is a NOTICE.
//   3. DISCARD — user data is dropped rather than adjusted (files beyond the first, tokens that
//      failed a filter, characters past a cap). MUST ANNOUNCE WHAT IT KEPT AND WHAT IT DROPPED.
//      Counting is the point: "kept 1, ignored 2" is actionable, "one file appeared" is not.
//
// The dividing question is always the same: could the user tell? If yes, tell them first.
//
// ── The oracle ─────────────────────────────────────────────────────────────────────────────
// The rendered text of the control's live region after a real blur / a real drop. Not a prop,
// not a callback spy: a callback the caller may ignore is exactly how a clamp stays silent.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { InputNumber } from './input-number'
import { Upload } from './upload'
import { Pagination } from './pagination'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
  globalThis.matchMedia ??= ((q: string) => ({
    matches: false, media: q, onchange: null,
    addListener() {}, removeListener() {},
    addEventListener() {}, removeEventListener() {},
    dispatchEvent() { return false },
  })) as never
})

afterEach(cleanup)

/** What the control announces — the ONE live region a user (or a screen reader) is told through. */
const announced = (): string =>
  Array.from(document.querySelectorAll('[data-slot="clamp-notice"]'))
    .map(n => (n.textContent ?? '').replace(/\s+/g, ' ').trim())
    .filter(Boolean)
    .join('|')

const field = (testid: string): HTMLInputElement => {
  const el = document.querySelector<HTMLInputElement>(`[data-testid="${testid}"]`)
  if (!el) throw new Error(`no [data-testid="${testid}"]`)
  return el
}

async function typeThenBlur(el: HTMLInputElement, text: string): Promise<void> {
  await act(async () => {
    fireEvent.change(el, { target: { value: text } })
  })
  await act(async () => {
    fireEvent.blur(el)
  })
}

describe('InputNumber — a value rewritten on blur says so', () => {
  it('a value above the maximum is clamped AND announced, naming the bound', async () => {
    render(<InputNumber min={1} max={100} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), '999')
    expect(field('n').value, 'the clamp itself is unchanged — it is the silence that was the bug').toBe('100')
    expect(announced()).toMatch(/100/)
    expect(announced().toLowerCase()).toMatch(/max/)
  })

  it('a value below the minimum is clamped AND announced', async () => {
    render(<InputNumber min={5} max={100} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), '2')
    expect(field('n').value).toBe('5')
    expect(announced()).toMatch(/5/)
    expect(announced().toLowerCase()).toMatch(/min/)
  })

  it('a value rounded away by `precision` is announced — lost decimals are lost data', async () => {
    render(<InputNumber precision={2} data-testid="n" aria-label="Ratio" />)
    await typeThenBlur(field('n'), '1.2345')
    expect(field('n').value).toBe('1.23')
    expect(announced()).not.toBe('')
    expect(announced()).toMatch(/1\.23/)
  })

  it('an unparseable entry REVERTED to the previous value is announced — the worst silent case', async () => {
    // The user typed something, it vanished, and the old number reappeared as if they had never
    // touched it. Nothing on screen distinguishes that from "my keystrokes were not registered".
    render(<InputNumber value={7} onChange={() => {}} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), 'abc')
    expect(field('n').value).toBe('7')
    expect(announced()).not.toBe('')
    expect(announced()).toMatch(/7/)
  })

  it('NEGATIVE CONTROL — a value INSIDE the bounds announces nothing', async () => {
    // Without this, "there is a notice" would pass just as well if the control announced on every
    // blur, which is noise and trains the user to ignore the one that matters.
    render(<InputNumber min={1} max={100} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), '42')
    expect(field('n').value).toBe('42')
    expect(announced()).toBe('')
  })

  it('NEGATIVE CONTROL — a lossless normalization stays silent (`"1."` is 1, not a clamp)', async () => {
    render(<InputNumber min={0} max={100} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), '1.')
    expect(announced(), 'the value means the same thing; announcing it would be noise').toBe('')
  })

  it('the notice CLEARS when the user edits again — a stale notice describes a value that is gone', async () => {
    render(<InputNumber min={1} max={100} data-testid="n" aria-label="Count" />)
    await typeThenBlur(field('n'), '999')
    expect(announced()).not.toBe('')
    await act(async () => {
      fireEvent.change(field('n'), { target: { value: '5' } })
    })
    expect(announced()).toBe('')
  })

  it('the notice lives in a LIVE REGION, so it reaches a user who cannot see it', async () => {
    render(<InputNumber min={1} max={100} data-testid="n" aria-label="Count" />)
    const region = document.querySelector('[data-slot="clamp-notice"]')
    expect(region, 'the region must exist BEFORE the message — a node inserted WITH its text is not reliably announced').not.toBeNull()
    expect(region?.getAttribute('role')).toBe('status')
    expect(region?.getAttribute('aria-live')).toBe('polite')
  })
})

describe('Upload — files dropped and thrown away say how many', () => {
  function dropFiles(zone: Element, names: string[]): Promise<void> {
    const files = names.map(n => new File(['x'], n, { type: 'text/plain' }))
    return act(async () => {
      fireEvent.drop(zone, { dataTransfer: { files } })
    })
  }

  const zone = (): Element => {
    const el = document.querySelector('[data-testid="up"] [role="button"]')
    if (!el) throw new Error('no dropzone')
    return el
  }

  it('a single-file dropzone given three files says it kept one and ignored two', async () => {
    const got: File[][] = []
    render(
      <Upload onFiles={f => got.push(f)} label="Pick a file" data-testid="up">
        <span>Drop here</span>
      </Upload>,
    )
    await dropFiles(zone(), ['a.txt', 'b.txt', 'c.txt'])
    expect(got.length, 'the behaviour is unchanged — one file is still what the caller gets').toBe(1)
    expect(got[0]?.length).toBe(1)
    expect(got[0]?.[0]?.name).toBe('a.txt')
    expect(announced(), 'and the two that were thrown away are now stated').toMatch(/2/)
    expect(announced()).toMatch(/a\.txt/)
  })

  it('NEGATIVE CONTROL — a single-file dropzone given ONE file announces nothing', async () => {
    render(
      <Upload onFiles={() => {}} label="Pick a file" data-testid="up">
        <span>Drop here</span>
      </Upload>,
    )
    await dropFiles(zone(), ['a.txt'])
    expect(announced()).toBe('')
  })

  it('NEGATIVE CONTROL — a MULTIPLE dropzone keeps every file and announces nothing', async () => {
    const got: File[][] = []
    render(
      <Upload multiple onFiles={f => got.push(f)} label="Pick files" data-testid="up">
        <span>Drop here</span>
      </Upload>,
    )
    await dropFiles(zone(), ['a.txt', 'b.txt', 'c.txt'])
    expect(got[0]?.length).toBe(3)
    expect(announced()).toBe('')
  })

  it('the notice lives in a LIVE REGION', async () => {
    render(
      <Upload onFiles={() => {}} label="Pick a file" data-testid="up">
        <span>Drop here</span>
      </Upload>,
    )
    const region = document.querySelector('[data-testid="up"] [data-slot="clamp-notice"]')
    expect(region).not.toBeNull()
    expect(region?.getAttribute('role')).toBe('status')
    expect(region?.getAttribute('aria-live')).toBe('polite')
  })
})

describe('Pagination quick-jumper — three outcomes, three answers', () => {
  const Jumper = ({ pageCount }: { pageCount: number }) => {
    const [page, setPage] = React.useState(1)
    return (
      <Pagination
        current={page}
        onChange={setPage}
        total={pageCount * 10}
        pageSize={10}
        showQuickJumper
        jumpLabel="Go to page"
        pageLabel={n => `Page ${n}`}
        previousLabel="Previous"
        nextLabel="Next"
        aria-label="Pagination"
        data-testid="pg"
      />
    )
  }
  const jump = (): HTMLInputElement => field('pg-jump')

  it('a page past the end says how many pages there are, and where it went', async () => {
    render(<Jumper pageCount={12} />)
    await typeThenBlur(jump(), '999')
    expect(announced()).toMatch(/12/)
  })

  it('an entry that is not a page number is REFUSED out loud, not discarded in silence', async () => {
    // The worst of the three: the box empties and nothing moves, which is exactly what a broken
    // control looks like.
    render(<Jumper pageCount={12} />)
    await typeThenBlur(jump(), '3.5')
    expect(announced()).not.toBe('')
    expect(announced()).toMatch(/3\.5/)
  })

  it('NEGATIVE CONTROL — a valid in-range page announces nothing', async () => {
    render(<Jumper pageCount={12} />)
    await typeThenBlur(jump(), '4')
    expect(announced()).toBe('')
  })

  it('NEGATIVE CONTROL — an empty box on blur announces nothing', async () => {
    render(<Jumper pageCount={12} />)
    await typeThenBlur(jump(), '')
    expect(announced()).toBe('')
  })
})
