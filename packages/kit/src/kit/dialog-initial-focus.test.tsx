// @vitest-environment jsdom
//
// A DIALOG DECLARES WHERE FOCUS LANDS, AND NEVER LANDS IT ON A DESTRUCTIVE CONTROL.
//
// ── The defect ─────────────────────────────────────────────────────────────────────────────
// Base UI's Dialog moves focus to the FIRST TABBABLE ELEMENT in the popup when it opens. That
// is a DOM-order rule, so which control it picks is decided by layout, not by intent. In
// `ShareDialog` the list of existing shares renders ABOVE the form, so the first tabbable is a
// per-row bin — "Remove access for <principal>" — and the dialog opens with a destructive
// control focused and one Enter away.
//
// It reads as fine today ONLY because the shares load asynchronously: at the instant the dialog
// mounts the list is a spinner, so the first tabbable is the role Select, and by the time the
// rows arrive focus has already been placed. That is TIMING, not design. A cached/synchronous
// store, a slower render, a test double — any of them flips it. It was found while DISPROVING a
// suspected tooltip bug: the "already-open tooltip" turned out to be a programmatic focus on
// that same bin reporting `:focus-visible`.
//
// ── Why the fix is a kit seam and not a per-dialog `autoFocus` ──────────────────────────────
// `autoFocus` on one input fixes one dialog and leaves the rule unstated, so the next dialog
// whose first tabbable happens to be a delete button reintroduces it silently. Two parts:
//
//   1. `initialFocus` — the dialog SAYS where focus belongs (a `data-testid`, a ref, or `false`
//      for "leave it on the popup"). Intent, declared, not inferred from DOM order.
//   2. THE DEFAULT IS GUARDED. With nothing declared, the kit still picks the first tabbable —
//      but SKIPS anything marked `data-destructive`, and if every candidate is destructive it
//      focuses the popup itself rather than a delete button. So a dialog that declares nothing
//      is safe, which is the only way a default is worth having.
//
// `Button` stamps `data-destructive` for `variant="destructive"` automatically, and takes a
// `destructive` prop for the ones that are destructive but styled QUIETLY — the ghost bin icon
// is exactly that case, and is why variant alone could not have been the signal.
//
// ── The oracle ─────────────────────────────────────────────────────────────────────────────
// `document.activeElement` after opening. Not a prop, not a source read: this class exists
// because every call site's source reads as compliant.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { act, cleanup, render } from '@testing-library/react'
import { Button } from './button'
import { Dialog } from './dialog'
import { Confirm } from './confirm'

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
  ;(Element.prototype as { getAnimations?: () => unknown[] }).getAnimations ??= () => []
  if (!globalThis.CSS) (globalThis as { CSS?: unknown }).CSS = { escape: (v: string) => v }
})

afterEach(cleanup)

/** Let the open transition + base-ui's focus effect run. */
async function settle(): Promise<void> {
  await act(async () => {
    await new Promise(r => setTimeout(r, 60))
  })
}

const focusedTestid = (): string | null =>
  document.activeElement?.getAttribute('data-testid') ?? null

/** The exposure, reproduced: a destructive row control that PRECEDES the form in DOM order. */
function DialogWithLeadingDestructive(props: {
  initialFocus?: string | React.RefObject<HTMLElement | null> | false
  markDestructive?: boolean
}) {
  return (
    <Dialog
      open
      title="Share dataset"
      data-testid="dlg"
      initialFocus={props.initialFocus}
    >
      <Button
        size="icon"
        variant="ghost"
        tooltip="Remove access for alice"
        destructive={props.markDestructive}
        data-testid="row-remove"
      >
        <span aria-hidden>x</span>
      </Button>
      <input data-testid="principal" aria-label="Principal" />
      <Button data-testid="save">Save</Button>
    </Dialog>
  )
}

describe('a dialog never opens with focus on a destructive control', () => {
  it('REPRODUCES THE EXPOSURE — an UNMARKED leading control still takes focus (DOM order wins)', async () => {
    // The negative control for the guard: this is what every dialog does today, and it is only
    // safe when nothing destructive happens to sort first. Kept so a future change that made the
    // guard fire for EVERY first tabbable (i.e. never focus anything) cannot pass as a fix.
    render(<DialogWithLeadingDestructive />)
    await settle()
    expect(focusedTestid()).toBe('row-remove')
  })

  it('a control declared destructive is SKIPPED, and the next candidate takes focus', async () => {
    render(<DialogWithLeadingDestructive markDestructive />)
    await settle()
    expect(
      focusedTestid(),
      'the bin is destructive; focus must fall through to the first non-destructive control',
    ).toBe('principal')
  })

  it('variant="destructive" is signal enough — a caller need not say it twice', async () => {
    render(
      <Dialog open title="Delete" data-testid="dlg">
        <Button variant="destructive" data-testid="del">Delete forever</Button>
        <input data-testid="confirm-name" aria-label="Name" />
      </Dialog>,
    )
    await settle()
    expect(focusedTestid()).toBe('confirm-name')
  })

  it('when EVERY candidate is destructive, focus goes to the popup — never to a delete button', async () => {
    render(
      <Dialog open title="Purge" data-testid="dlg-only-destructive">
        <Button variant="destructive" data-testid="del-a">Delete A</Button>
        <Button variant="destructive" data-testid="del-b">Delete B</Button>
      </Dialog>,
    )
    await settle()
    // The corner × is a close affordance, not a candidate we would ever choose over the popup,
    // and it is certainly not a delete button — assert the negative that matters.
    expect(focusedTestid()).not.toBe('del-a')
    expect(focusedTestid()).not.toBe('del-b')
    expect(
      document.activeElement?.closest('[data-testid="dlg-only-destructive"]'),
      'focus must stay INSIDE the dialog — a focus trap that focuses nothing is its own bug',
    ).not.toBeNull()
  })
})

describe('a dialog can DECLARE where focus lands', () => {
  it('a data-testid string focuses that element, over the DOM-order default', async () => {
    render(<DialogWithLeadingDestructive initialFocus="save" />)
    await settle()
    expect(focusedTestid()).toBe('save')
  })

  it('a ref focuses that element', async () => {
    const ref = React.createRef<HTMLInputElement>()
    render(
      <Dialog open title="Ref" data-testid="dlg" initialFocus={ref}>
        <Button data-testid="first">First</Button>
        <input ref={ref} data-testid="target" aria-label="Target" />
      </Dialog>,
    )
    await settle()
    expect(focusedTestid()).toBe('target')
  })

  it('`false` leaves focus on the popup, not on the first control', async () => {
    render(<DialogWithLeadingDestructive initialFocus={false} />)
    await settle()
    expect(focusedTestid()).not.toBe('row-remove')
    expect(focusedTestid()).not.toBe('principal')
  })

  it('a declared target that is not in the DOM falls back to the GUARDED default, not to the destructive control', async () => {
    // A stale testid after a rename must degrade to the safe default, never to "whatever sorts
    // first" — otherwise the seam becomes a way to silently re-acquire the exposure.
    render(<DialogWithLeadingDestructive initialFocus="typo-not-here" markDestructive />)
    await settle()
    expect(focusedTestid()).toBe('principal')
  })
})

describe('Confirm — the prompt whose whole job is a destructive choice', () => {
  it('opens with Cancel focused, never the destructive confirm', async () => {
    render(
      <Confirm
        open
        danger
        title="Delete project"
        okText="Delete"
        cancelText="Cancel"
        onConfirm={() => {}}
        data-testid="cf"
      />,
    )
    await settle()
    expect(focusedTestid()).toBe('cf-cancel')
  })
})

describe("Confirm's Cancel — the control a user reaches for when a confirm is taking too long", () => {
  // CONTROLLED BY REAL STATE, not a pinned `open` — a dismissal test against a hardcoded
  // `open` would assert nothing (the dialog cannot close), and would pass on any source.
  const Host = ({ onConfirm }: { onConfirm: () => Promise<void> | void }) => {
    const [open, setOpen] = React.useState(true)
    return (
      <Confirm
        open={open}
        onOpenChange={setOpen}
        danger
        title="Delete project"
        description="This cannot be undone."
        okText="Delete"
        cancelText="Cancel"
        onConfirm={onConfirm}
        data-testid="cf"
      />
    )
  }
  const openConfirm = (onConfirm: () => Promise<void> | void) => render(<Host onConfirm={onConfirm} />)
  const el = (id: string): HTMLElement => {
    const n = document.querySelector<HTMLElement>(`[data-testid="${id}"]`)
    if (!n) throw new Error(`no [data-testid="${id}"]`)
    return n
  }

  it('still DISMISSES when pressed — the reason plumbing must not cost the button its job', async () => {
    // Cancel is now a kit Button handed to `AlertDialogCancel` via `render`. That is the only way
    // it can carry `unavailableReason`, and it is also the way to break dismissal silently, so
    // this drives the click rather than reading the tree.
    openConfirm(() => {})
    await settle()
    await act(async () => {
      el('cf-cancel').click()
    })
    await settle()
    expect(document.querySelector('[data-testid="cf"]')).toBeNull()
  })

  it('WHILE A CONFIRM IS IN FLIGHT it says what it is waiting for, and stays a tab stop', async () => {
    // Previously `disabled={busy}`: pointer-events:none, out of the tab order, no explanation —
    // while Esc and the backdrop still dismissed, so the refusal was not even true of the DIALOG,
    // only of this one button.
    let release: (() => void) | undefined
    openConfirm(() => new Promise<void>(r => { release = r }))
    await settle()
    await act(async () => {
      el('cf-confirm').click()
    })
    await settle()
    const cancel = el('cf-cancel')
    expect(cancel.getAttribute('aria-disabled')).toBe('true')
    expect(
      cancel.hasAttribute('disabled'),
      'a natively disabled button is out of the tab order — the keyboard user loses it entirely',
    ).toBe(false)
    const described = (cancel.getAttribute('aria-describedby') ?? '')
      .split(/\s+/).filter(Boolean)
      .map(id => document.getElementById(id)?.textContent ?? '').join(' ').trim()
    expect(described, 'the reason must reach assistive tech').not.toBe('')
    await act(async () => { release?.() })
  })
})
