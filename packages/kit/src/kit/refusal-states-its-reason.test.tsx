// @vitest-environment jsdom
//
// A CONTROL THAT REFUSES MUST SAY WHY — and the kit must not be the one withholding it.
//
// `icon-button-tooltip.test.tsx` closed the case where a reason EXISTED and could not be REACHED
// (a natively `disabled` button is pointer-events:none and out of the tab order, so a tooltip
// parked on one can never open). This file covers the strictly worse case underneath it: the
// reason does not exist in ANY channel, so there is nothing to make reachable.
//
// Two kit-owned refusals are in scope, and they are kit-owned in different senses:
//
//   1. THE COLUMN CHOOSER'S LAST VISIBLE COLUMN. The kit enforces a rule it invented and never
//      states — "a table must keep at least one visible column" (`canHideColumn`). It is enforced
//      TWICE (the Checkbox is disabled in the toolbar, and `toggleHidden` silently declines), and
//      written down only in two code comments. The user's whole channel is a greyed tick box
//      inside a popover, where no adjacent explanatory text is even possible.
//
//   2. `Checkbox` HAD NO WAY TO CARRY A REASON AT ALL. Only `Button` had `unavailableReason`, so
//      the chooser could not have explained itself even if its author had wanted to. Fixing the
//      call site without the prop would have meant hand-rolling the five coupled parts
//      (aria-disabled, dropped handler, the disabled look, the tooltip, and a document-unique
//      `aria-describedby` target) a second time — which is exactly the reasoning that made
//      `unavailableReason` a prop rather than a documented recipe.
//
// EVERY ASSERTION READS THE RENDERED DOM. Not props, not source text: this class exists precisely
// because the source of every one of these sites reads as compliant. And deliberately NOT a
// console spy — base-ui and the kit both dedupe messages process-wide, so a spy can pass because
// an unrelated earlier test consumed the message.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { Button } from './button'
import { Checkbox } from './checkbox'
import { Switch } from './switch'
import { Table } from './table'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
  globalThis.matchMedia ??= ((q: string) => ({
    matches: false,
    media: q,
    onchange: null,
    addListener() {},
    removeListener() {},
    addEventListener() {},
    removeEventListener() {},
    dispatchEvent() {
      return false
    },
  })) as never
  ;(Element.prototype as { getAnimations?: () => unknown[] }).getAnimations ??= () => []
  // jsdom ships no `PointerEvent`, and base-ui's CheckboxRoot CONSTRUCTS one inside its click
  // handler (`new (ownerWindow(el).PointerEvent)(…)`) to re-dispatch to the hidden native input.
  // Without this every `fireEvent.click` on a Checkbox throws — an environment gap, not a
  // subject behaviour, so it is polyfilled here rather than worked around in the assertions.
  globalThis.PointerEvent ??= class extends MouseEvent {
    constructor(type: string, init?: MouseEventInit) {
      super(type, init)
    }
  } as never
})

afterEach(cleanup)

const byTestId = (id: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-testid="${id}"]`)
  if (!el) throw new Error(`no element with data-testid="${id}"`)
  return el
}

/** Hover the way a pointer does, then let the open delay elapse. */
async function hover(el: Element): Promise<void> {
  await act(async () => {
    fireEvent.pointerEnter(el, { pointerType: 'mouse', bubbles: true })
    fireEvent.mouseOver(el, { bubbles: true })
    fireEvent.mouseEnter(el, { bubbles: true })
    fireEvent.mouseMove(el, { bubbles: true })
  })
  await act(async () => {
    await new Promise(r => setTimeout(r, 700))
  })
}

/** What a SIGHTED user reads on hover. '' when nothing opened. */
const shownTooltip = (): string =>
  Array.from(document.querySelectorAll('[data-slot="tooltip-content"]'))
    .map(t => (t.textContent ?? '').replace(/\s+/g, ' ').trim())
    .join('|')

/** What a SCREEN-READER user is told about WHY, resolved through `aria-describedby` off the
 *  document the way the browser resolves it. '' when the control describes nothing. */
const describedText = (el: Element): string =>
  (el.getAttribute('aria-describedby') ?? '')
    .split(/\s+/)
    .filter(Boolean)
    .map(id => (document.getElementById(id)?.textContent ?? '').replace(/\s+/g, ' ').trim())
    .filter(Boolean)
    .join(' ')

/** The accessible NAME, read the way the consumer's DOM sweep reads it. */
const accessibleName = (el: Element): string =>
  (el.getAttribute('aria-label') ?? '').trim() ||
  (el.getAttribute('aria-labelledby') ?? '')
    .split(/\s+/)
    .filter(Boolean)
    .map(id => (document.getElementById(id)?.textContent ?? '').trim())
    .join(' ') ||
  (el.textContent ?? '').replace(/\s+/g, ' ').trim()

// ─────────────────────────────────────────────────────────────────────────────
describe('A — Checkbox can carry the reason it refuses, and carries it to BOTH kinds of user', () => {
  it('opens the REASON on hover — which a natively `disabled` checkbox cannot do at all', async () => {
    render(
      <Checkbox
        checked
        onCheckedChange={() => {}}
        label="Sample"
        unavailableReason="A table must show at least one column"
        data-testid="a1"
      />,
    )
    await hover(byTestId('a1'))
    expect(shownTooltip()).toBe('A table must show at least one column')
  })

  it('also puts the reason in the accessibility tree, for someone who cannot hover', () => {
    render(
      <Checkbox
        checked
        onCheckedChange={() => {}}
        label="Sample"
        unavailableReason="A table must show at least one column"
        data-testid="a2"
      />,
    )
    expect(describedText(byTestId('a2'))).toBe('A table must show at least one column')
  })

  it('is aria-disabled and NOT natively disabled — the whole point, since native disable is what makes a reason unreachable', () => {
    render(
      <Checkbox
        checked
        onCheckedChange={() => {}}
        label="Sample"
        unavailableReason="A table must show at least one column"
        data-testid="a3"
      />,
    )
    const el = byTestId('a3')
    expect(el.getAttribute('aria-disabled')).toBe('true')
    expect(el.hasAttribute('disabled')).toBe(false)
  })

  it('still REFUSES: aria-disabled is a claim, so the toggle must actually be swallowed', () => {
    let toggles = 0
    render(
      <Checkbox
        checked
        onCheckedChange={() => {
          toggles += 1
        }}
        label="Sample"
        unavailableReason="A table must show at least one column"
        data-testid="a4"
      />,
    )
    fireEvent.click(byTestId('a4'))
    expect(toggles).toBe(0)
  })

  it('keeps the NAME saying WHICH control refused, not why', () => {
    render(
      <Checkbox
        checked
        onCheckedChange={() => {}}
        aria-label="Toggle column Sample"
        unavailableReason="A table must show at least one column"
        data-testid="a5"
      />,
    )
    // The 93167985 rule: on a list of eight tick boxes, a reason that swallows the name leaves a
    // screen-reader user unable to tell which one refused them.
    expect(accessibleName(byTestId('a5'))).toBe('Toggle column Sample')
  })

  it('an EMPTY reason is not a reason — it must not silently make the control aria-disabled', () => {
    let toggles = 0
    render(
      <Checkbox
        checked
        onCheckedChange={() => {
          toggles += 1
        }}
        label="Sample"
        unavailableReason=""
        data-testid="a6"
      />,
    )
    const el = byTestId('a6')
    expect(el.getAttribute('aria-disabled')).toBe(null)
    fireEvent.click(el)
    expect(toggles).toBe(1)
  })

  it('TWO refused checkboxes each announce THEIR OWN reason — the id must be document-unique', () => {
    render(
      <>
        <Checkbox
          checked
          onCheckedChange={() => {}}
          label="One"
          unavailableReason="reason for the first"
          data-testid="a7a"
        />
        <Checkbox
          checked
          onCheckedChange={() => {}}
          label="Two"
          unavailableReason="reason for the second"
          data-testid="a7b"
        />
      </>,
    )
    // A single instance passes even with a hand-built constant id; only two can catch it.
    expect(describedText(byTestId('a7a'))).toBe('reason for the first')
    expect(describedText(byTestId('a7b'))).toBe('reason for the second')
  })

  it('a caller`s own aria-describedby is KEPT alongside the reason, never replaced', () => {
    render(
      <>
        <span id="ext-note">an external note</span>
        <Checkbox
          checked
          onCheckedChange={() => {}}
          label="Sample"
          aria-describedby="ext-note"
          unavailableReason="the reason"
          data-testid="a8"
        />
      </>,
    )
    const text = describedText(byTestId('a8'))
    expect(text).toContain('an external note')
    expect(text).toContain('the reason')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
describe('B — the column chooser states the rule it enforces', () => {
  const columns = [
    { key: 'a', title: 'Alpha' },
    { key: 'b', title: 'Beta' },
  ]
  const rows = [{ a: '1', b: '2' }]

  /** Open the chooser popover and return the per-column toggles. */
  async function openChooser(): Promise<void> {
    await act(async () => {
      fireEvent.click(byTestId('t-columns-btn'))
    })
    await act(async () => {
      await new Promise(r => setTimeout(r, 60))
    })
  }

  it('the LAST visible column`s toggle says why it cannot be unticked', async () => {
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    // hide one, leaving exactly one visible
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-b'))
    })
    const last = byTestId('t-col-toggle-a')
    await hover(last)
    expect(
      shownTooltip(),
      'the rule "a table must keep at least one visible column" is real, kit-invented, and was ' +
        'stated only in two code comments',
    ).toMatch(/at least one/i)
  })

  it('…and tells a screen-reader user the same thing', async () => {
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-b'))
    })
    expect(describedText(byTestId('t-col-toggle-a'))).toMatch(/at least one/i)
  })

  it('the refused toggle is still a TAB STOP, so a keyboard user reaches it and hears the reason', async () => {
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-b'))
    })
    const last = byTestId('t-col-toggle-a')
    expect(last.hasAttribute('disabled'), 'never NATIVELY disabled — that is what kills the tooltip').toBe(false)
    // The discriminating half. base-ui's Checkbox already avoids the native attribute, so
    // `hasAttribute('disabled')` alone passes against the UNFIXED source too; what native-disabled
    // semantics actually cost is the tab order, and that is what has to be asserted.
    expect(last.getAttribute('tabindex'), 'a refused control a keyboard user cannot reach explains nothing').toBe('0')
  })

  it('and it still REFUSES — clicking the last visible column does not hide it', async () => {
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-b'))
    })
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-a'))
    })
    // still ticked ⇒ still visible
    expect(byTestId('t-col-toggle-a').getAttribute('aria-checked')).toBe('true')
  })

  it('an ALREADY-HIDDEN column carries no reason — re-showing it is never refused', async () => {
    // The guard is two-part (`!isHidden(key) && visibleCount <= 1`); dropping the first half
    // leaves the arithmetic correct and puts the sentence on the wrong tick box — the one whose
    // tick RESTORES a column, which is never refused. A single-column assertion cannot see that.
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    await act(async () => {
      fireEvent.click(byTestId('t-col-toggle-b'))
    })
    const hidden = byTestId('t-col-toggle-b')
    expect(hidden.getAttribute('aria-checked')).toBe('false')
    expect(hidden.getAttribute('aria-disabled')).toBe(null)
    expect(describedText(hidden)).toBe('')
  })

  it('a column that CAN be hidden carries no reason — the reason marks refusal, nothing else', async () => {
    render(
      <Table
        data-testid="t"
        columnChooser
        columns={columns}
        dataSource={rows}
        rowKey={(r: Record<string, string>) => r.a}
      />,
    )
    await openChooser()
    const b = byTestId('t-col-toggle-b')
    expect(b.getAttribute('aria-disabled')).toBe(null)
    expect(describedText(b)).toBe('')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
describe('C — a declared tooltip is not silently discarded', () => {
  it('a Switch with BOTH a label and a tooltip still opens the tooltip', async () => {
    // Adjacent to this class rather than in it: the reason existed, was declared by the caller,
    // and the kit threw it away on one branch. `switch.tsx` built `maybeTip` and then returned
    // the BARE control whenever `label` was present, so the prop's documented promise held for
    // exactly half its call sites — and a label is precisely when the tooltip is carrying the
    // extra sentence rather than the name.
    render(
      <Switch
        checked
        onCheckedChange={() => {}}
        label="Allow ownership transfer"
        tooltip="Only the project owner can change this"
        data-testid="c1"
      />,
    )
    await hover(byTestId('c1'))
    expect(shownTooltip()).toBe('Only the project owner can change this')
  })
})

// ─────────────────────────────────────────────────────────────────────────────
describe('D — the reason never becomes the NAME, on a TEXT button either', () => {
  it('a text Button with a reason and no aria-label is still named by its words', () => {
    // The `sr-only` reason node lives INSIDE the button (a sibling wrapper would change every
    // flex rail's layout), and a button with no `aria-label` is named by its TEXT CONTENT — which
    // includes that node. So this rendered as "NextFill in Delta option". It survived because
    // every shipped caller was either an icon button (named by its string `tooltip`) or a text
    // button that had been handed an explicit `aria-label`, and because the prop's own doc
    // asserted the label was "always set" on the strength of the icon-only path alone.
    render(
      <Button unavailableReason="Fill in Delta option" data-testid="d1">
        Next
      </Button>,
    )
    const el = byTestId('d1')
    expect(el.getAttribute('aria-label')).toBe('Next')
    expect(accessibleName(el)).toBe('Next')
  })

  it('an explicit aria-label still wins over the derived one', () => {
    render(
      <Button aria-label="Run analysis" unavailableReason="Select frame" data-testid="d2">
        Run
      </Button>,
    )
    expect(accessibleName(byTestId('d2'))).toBe('Run analysis')
  })

  it('an AVAILABLE text button is left exactly as it was — no label invented', () => {
    render(<Button data-testid="d3">Next</Button>)
    // The fallback is scoped to the unavailable case on purpose: naming every text button by its
    // own words is a no-op at best and, where a caller relies on content-derived naming through
    // a translated child component, a silent freeze of the untranslated string.
    expect(byTestId('d3').getAttribute('aria-label')).toBe(null)
  })
})
