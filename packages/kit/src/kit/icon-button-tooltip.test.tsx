// @vitest-environment jsdom
//
// AN ICON-ONLY CONTROL MUST SAY WHAT IT DOES — asserted by OPENING the tooltip, never by
// reading a prop.
//
// The consuming app (CytoAnalyst, commit 265397a8) swept 94 rendered gallery surfaces and found
// 279 visible icon-only controls, 127 of them mute. Three of the causes live in THIS package and
// are invisible to any static check, which is precisely why they survived review:
//
//   A. `iconOnly` was `icon != null && children == null`, so
//          <Button size="icon" aria-label="Close"><X /></Button>
//      satisfied the prop union AND KIT_MANIFEST.md and rendered no tooltip. Source compliant,
//      DOM mute.
//
//   B. A kit Button used as somebody else's Base UI trigger (a Popover's, a Dropdown's, a
//      Confirm's) still STAMPED `data-base-ui-tooltip-trigger`, so markers and source both read
//      fine, while hovering it opened nothing. The app carried a `TriggerTooltip` wrapper for
//      months to work around it.
//
//      THE MECHANISM, measured here rather than guessed: `Tooltip.Trigger` keys its store on
//      `useBaseUiId(idProp)` — the `id` prop it was GIVEN — and then base-ui merges the rendered
//      element's own props over its computed ones. The outer trigger injects an `id`, that `id`
//      lands on the DOM node, and the tooltip is left registered under an id nothing on screen
//      carries. Bisected one prop at a time against a bare `TooltipTrigger`: `aria-haspopup`,
//      `aria-expanded`, `data-base-ui-click-trigger`, `data-slot`, `tabIndex` and every injected
//      handler are all harmless; `id` alone kills it. Hence `<TooltipTrigger id={...}>`.
//
//   C. A natively `disabled` button can open no React tooltip AT ALL: it is `pointer-events:none`
//      by the kit's own class, and even without that a disabled control's mouse events do not
//      reach the document root, which is where React listens. The kit therefore says so instead
//      of rendering something inert.
//
// WHY jsdom IS A LEGITIMATE ORACLE HERE. It reproduced both defects with the same verdicts the
// app measured in a real browser — the four composed sites open ZERO tooltips, their un-composed
// peers open one — and it reports the fix the same way. The browser-side proof is the consumer's
// `tests/gallery-e2e/icon-button-tooltips.spec.ts` sweep over 94 real surfaces; this file is what
// makes a regression cost seconds instead of a 25-minute sweep.

import { describe, it, expect, beforeAll, afterEach, vi } from 'vitest'
import * as React from 'react'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { Button } from './button'
import { Popover } from './popover'
import { Dropdown } from './dropdown'
import { Confirm } from './confirm'
import { Dialog } from './dialog'
import { Alert } from './alert'
import { Tag } from './tag'
import { PasswordInput } from './input'
import { Tooltip } from './tooltip'
import { Tooltip as ShTooltip, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'

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
})

afterEach(cleanup)

const Icon = () => <svg width="16" height="16" />

/** Hover a control the way a pointer does, then let the open delay elapse. */
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

const byTestId = (id: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-testid="${id}"]`)
  if (!el) throw new Error(`no element with data-testid="${id}"`)
  return el
}

/** The accessible name, read the way the consumer's sweep reads it. */
const accessibleName = (el: Element): string =>
  (el.getAttribute('aria-label') ?? '').trim() || (el.textContent ?? '').replace(/\s+/g, ' ').trim()

/** Open the control's tooltip and return what it says. */
async function tooltipOf(el: Element): Promise<string> {
  await hover(el)
  return shownTooltip()
}

describe('A — an icon-only Button is icon-only however the icon got there', () => {
  it('opens a tooltip when the icon is a CHILD and the name is aria-label', async () => {
    render(
      <Button size="icon" aria-label="Close" data-testid="a1">
        <Icon />
      </Button>,
    )
    expect(
      await tooltipOf(byTestId('a1')),
      '<Button size="icon" aria-label="Close"><X/></Button> type-checks, satisfies KIT_MANIFEST ' +
        "and used to render nothing on hover, because `iconOnly` required `children == null`",
    ).toBe('Close')
  })

  it('still opens one when the icon came through the `icon` prop (the path that always worked)', async () => {
    render(<Button size="icon" aria-label="Close" icon={<Icon />} data-testid="a2" />)
    expect(await tooltipOf(byTestId('a2'))).toBe('Close')
  })

  it('adopts the aria-label as the hover text for an icon child with no explicit tooltip', async () => {
    render(
      <Button size="icon" aria-label="Delete row" data-testid="a3">
        <Icon />
      </Button>,
    )
    const el = byTestId('a3')
    const shown = await tooltipOf(el)
    expect(shown, 'the two channels must say the same thing').toBe(accessibleName(el))
  })

  it('treats an icon-only button with no explicit size as icon-only too', async () => {
    // The defect is one prop away from `size="icon"`: the button below renders exactly the
    // same mute glyph, so keying icon-only-ness off the SIZE token alone would leave it.
    render(
      <Button aria-label="Dismiss" data-testid="a4">
        <Icon />
      </Button>,
    )
    expect(await tooltipOf(byTestId('a4'))).toBe('Dismiss')
  })
})

describe('A(negative) — a button that HAS words is not icon-only', () => {
  it('a text button gets no tooltip and is not a tooltip trigger', async () => {
    render(
      <Button aria-label="Save the form" data-testid="n1">
        Save
      </Button>,
    )
    const el = byTestId('n1')
    expect(el.hasAttribute('data-base-ui-tooltip-trigger'), 'a labelled button needs no hover text').toBe(false)
    expect(await tooltipOf(el)).toBe('')
  })

  it('an icon PLUS text is not icon-only', async () => {
    render(
      <Button icon={<Icon />} aria-label="Export as CSV" data-testid="n2">
        Export
      </Button>,
    )
    expect(await tooltipOf(byTestId('n2'))).toBe('')
  })

  it('text nested inside an element child still counts as text', async () => {
    render(
      <Button aria-label="Save the form" data-testid="n3">
        <span>
          <b>Save</b>
        </span>
      </Button>,
    )
    expect(
      await tooltipOf(byTestId('n3')),
      'a walk that only looked at direct children would call this icon-only and label it wrongly',
    ).toBe('')
  })

  it('text among an ARRAY of children still counts as text', async () => {
    // JSX with more than one child hands `children` down as an array, and `{n} items` — the
    // commonest interpolated label there is — is exactly that shape. A walk that stops at arrays
    // calls it icon-only and hangs the aria-label on it as hover text.
    const n = 3
    render(
      <Button aria-label="Three items selected" data-testid="n6b">
        {n} items
      </Button>,
    )
    expect(await tooltipOf(byTestId('n6b'))).toBe('')
  })

  it('a fragment of icons with no words IS icon-only', async () => {
    render(
      <Button aria-label="Merge" data-testid="n4">
        <>
          <Icon />
          <Icon />
        </>
      </Button>,
    )
    expect(await tooltipOf(byTestId('n4'))).toBe('Merge')
  })

  it('aria-hidden text is decoration, not a label', async () => {
    render(
      <Button aria-label="Sort" data-testid="n5">
        <span aria-hidden="true">↕</span>
      </Button>,
    )
    expect(
      await tooltipOf(byTestId('n5')),
      'aria-hidden declares "this is not the label"; taking it as one lets any glyph opt out',
    ).toBe('Sort')
  })

  it('an explicit tooltip is honoured on a button with words', async () => {
    render(
      <Button tooltip="Save the form" data-testid="n6">
        Save
      </Button>,
    )
    expect(await tooltipOf(byTestId('n6')), 'an explicit tooltip is a request, not an inference').toBe(
      'Save the form',
    )
  })
})

describe('B — a Button that is ALSO somebody else’s trigger still opens its tooltip', () => {
  it('inside a Popover trigger', async () => {
    render(
      <Popover content={<div>body</div>}>
        <Button size="icon" tooltip="Folders" icon={<Icon />} data-testid="b1" />
      </Popover>,
    )
    expect(
      await tooltipOf(byTestId('b1')),
      'the button stamps data-base-ui-tooltip-trigger either way — only opening it proves anything',
    ).toBe('Folders')
  })

  it('inside a Dropdown trigger', async () => {
    render(
      <Dropdown data-testid="dd" items={[{ key: 'a', label: 'A' }]}>
        <Button size="icon" tooltip="More actions" icon={<Icon />} data-testid="b2" />
      </Dropdown>,
    )
    expect(await tooltipOf(byTestId('b2'))).toBe('More actions')
  })

  it('inside a Confirm trigger', async () => {
    render(
      <Confirm data-testid="cf" title="Sure?" okText="Yes" cancelText="No" onConfirm={() => {}}>
        <Button size="icon" tooltip="Delete" icon={<Icon />} data-testid="b3" />
      </Confirm>,
    )
    expect(await tooltipOf(byTestId('b3'))).toBe('Delete')
  })

  it('with the icon as a CHILD and the name as aria-label — both defects at once', async () => {
    render(
      <Popover content={<div>body</div>}>
        <Button size="icon" aria-label="Folders" data-testid="b4">
          <Icon />
        </Button>
      </Popover>,
    )
    expect(await tooltipOf(byTestId('b4'))).toBe('Folders')
  })

  it('and the OTHER trigger still works — the tooltip must not cost the button its job', async () => {
    render(
      <Popover content={<div data-testid="pop-body">body</div>}>
        <Button size="icon" tooltip="Folders" icon={<Icon />} data-testid="b5" />
      </Popover>,
    )
    await act(async () => {
      fireEvent.click(byTestId('b5'))
    })
    expect(
      document.querySelector('[data-testid="pop-body"]'),
      'a fix that opened the tooltip by breaking the popover would be a worse bug',
    ).not.toBeNull()
  })

  it('the outer trigger’s own props still reach the DOM', async () => {
    render(
      <Popover content={<div>body</div>}>
        <Button size="icon" tooltip="Folders" icon={<Icon />} data-testid="b6" />
      </Popover>,
    )
    const el = byTestId('b6')
    expect(el.getAttribute('aria-haspopup'), 'the popover still owns the button').toBe('dialog')
    expect(el.getAttribute('data-slot')).toBe('popover-trigger')
  })

  it('the caller’s own id is preserved, not swallowed by the tooltip', async () => {
    render(<Button size="icon" tooltip="Solo" icon={<Icon />} id="my-own-id" data-testid="b7" />)
    expect(byTestId('b7').id).toBe('my-own-id')
    expect(await tooltipOf(byTestId('b7'))).toBe('Solo')
  })

  it('…and on a button with NO tooltip, where nothing else would put it back', () => {
    // `id` is destructured out of the prop bag so it can be hoisted onto the tooltip trigger.
    // On this path there IS no trigger, so the Button has to re-apply it itself or the caller's
    // id silently disappears — which would break every `htmlFor`/`aria-controls` pointing at it.
    render(
      <Button id="plain-id" data-testid="b8">
        Save
      </Button>,
    )
    expect(byTestId('b8').id).toBe('plain-id')
  })

  it('…and on an anchor Button, which is a third render path', () => {
    render(
      <Button href="https://example.com" id="link-id" data-testid="b9">
        Docs
      </Button>,
    )
    expect(byTestId('b9').id).toBe('link-id')
  })
})

describe('B(mechanism) — the base-ui contract this fix rests on', () => {
  /**
   * Pins the exact upstream behaviour the fix compensates for. If a base-ui upgrade ever makes
   * `Tooltip.Trigger` resolve its store key from the RENDERED id instead of the id PROP, this
   * test flips and the `id` pass-through can be deleted rather than carried forever.
   */
  it('an id on the rendered element, not passed to Tooltip.Trigger, orphans the tooltip', async () => {
    render(
      <TooltipProvider delay={300}>
        <ShTooltip>
          <TooltipTrigger
            render={
              <button type="button" data-testid="m1" aria-label="F" id="injected-by-another-trigger">
                <Icon />
              </button>
            }
          />
          <TooltipContent>Folders</TooltipContent>
        </ShTooltip>
      </TooltipProvider>,
    )
    expect(await tooltipOf(byTestId('m1'))).toBe('')
  })

  it('…and passing that same id to Tooltip.Trigger fixes it', async () => {
    render(
      <TooltipProvider delay={300}>
        <ShTooltip>
          <TooltipTrigger
            id="injected-by-another-trigger"
            render={
              <button type="button" data-testid="m2" aria-label="F" id="injected-by-another-trigger">
                <Icon />
              </button>
            }
          />
          <TooltipContent>Folders</TooltipContent>
        </ShTooltip>
      </TooltipProvider>,
    )
    expect(await tooltipOf(byTestId('m2'))).toBe('Folders')
  })
})

describe('B(kit Tooltip) — an outer <Tooltip> composes onto a foreign trigger too', () => {
  it('opens when the kit <Tooltip> wraps a Button that is a Popover trigger', async () => {
    render(
      <Popover content={<div>body</div>}>
        <Tooltip content="Folders">
          <Button size="icon" aria-label="Folders" data-testid="t1">
            <Icon />
          </Button>
        </Tooltip>
      </Popover>,
    )
    expect(
      await tooltipOf(byTestId('t1')),
      'exactly ONE tooltip may open — the wrapper owns it, the button must not add a second',
    ).toBe('Folders')
  })
})

describe('D — the controls the KIT draws explain themselves too', () => {
  it("the Dialog's close button", async () => {
    render(
      <Dialog open title="Settings" data-testid="d1">
        body
      </Dialog>,
    )
    const close = document.querySelector<HTMLElement>('[data-slot="dialog-close"]')
    expect(close, 'the dialog must render a close button').not.toBeNull()
    const shown = await tooltipOf(close!)
    expect(shown, 'nine gallery surfaces showed this exact glyph with nothing on hover').toBe('Close')
    expect(shown, 'and it must say what the control is CALLED').toBe(accessibleName(close!))
  })

  it("the Dialog's close button uses the caller's word when given one", async () => {
    render(
      <Dialog open title="Settings" closeLabel="Dismiss" data-testid="d2">
        body
      </Dialog>,
    )
    const close = document.querySelector<HTMLElement>('[data-slot="dialog-close"]')!
    expect(await tooltipOf(close)).toBe('Dismiss')
    expect(accessibleName(close), 'both channels move together').toBe('Dismiss')
  })

  it("the Alert's dismiss button", async () => {
    render(<Alert data-testid="al" title="Nope" onClose={() => {}} closeLabel="Dismiss this alert" />)
    const el = byTestId('al-close')
    expect(await tooltipOf(el)).toBe('Dismiss this alert')
    expect(accessibleName(el)).toBe('Dismiss this alert')
  })

  it("the PasswordInput's reveal toggle, in BOTH states", async () => {
    render(<PasswordInput data-testid="pw" showLabel="Show password" hideLabel="Hide password" />)
    const toggle = document.querySelector<HTMLElement>('[data-slot="password-reveal"]')
    expect(toggle, 'the password field must render a reveal toggle').not.toBeNull()
    expect(await tooltipOf(toggle!)).toBe('Show password')

    await act(async () => {
      fireEvent.click(toggle!)
    })
    await act(async () => {
      fireEvent.mouseLeave(toggle!, { bubbles: true })
      await new Promise(r => setTimeout(r, 200))
    })
    const after = document.querySelector<HTMLElement>('[data-slot="password-reveal"]')!
    expect(
      await tooltipOf(after),
      'a toggle whose hover text is stuck on the state it LEFT is worse than none',
    ).toContain('Hide password')
  })

  it("the Tag's remove button", async () => {
    render(
      <Tag data-testid="tg" onClose={() => {}} closeLabel="Remove Alpha">
        Alpha
      </Tag>,
    )
    const el = byTestId('tg-close')
    expect(await tooltipOf(el)).toBe('Remove Alpha')
    expect(accessibleName(el)).toBe('Remove Alpha')
  })
})

describe('C — a natively disabled button cannot open a tooltip, and the kit says so', () => {
  it('warns, naming the control and the two remedies', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    render(<Button size="icon" tooltip="No more analyses to add" icon={<Icon />} disabled data-testid="c1" />)
    const said = warn.mock.calls.map(c => c.join(' ')).join('\n')
    warn.mockRestore()
    expect(said, 'the warning must name the control it is about').toContain('c1')
    expect(said, 'and it must name the remedy, or it is just noise').toMatch(/aria-disabled/)
  })

  it('does not warn for an ENABLED tooltipped button', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    render(<Button size="icon" tooltip="Fine" icon={<Icon />} data-testid="c2" />)
    const said = warn.mock.calls.map(c => c.join(' ')).join('\n')
    warn.mockRestore()
    expect(said, 'a warning that fires on the healthy case is a warning nobody reads').not.toContain('c2')
  })

  it('does not warn for a disabled button with NO tooltip', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    render(
      <Button disabled data-testid="c3">
        Save
      </Button>,
    )
    const said = warn.mock.calls.map(c => c.join(' ')).join('\n')
    warn.mockRestore()
    expect(said).not.toContain('c3')
  })

  it('does not warn for `loading`, which is aria-disabled and CAN open its tooltip', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    render(<Button size="icon" tooltip="Saving" icon={<Icon />} loading data-testid="c4" />)
    const said = warn.mock.calls.map(c => c.join(' ')).join('\n')
    warn.mockRestore()
    expect(said, 'loading keeps the button focusable and hoverable on purpose').not.toContain('c4')
    expect(await tooltipOf(byTestId('c4'))).toBe('Saving')
  })
})
