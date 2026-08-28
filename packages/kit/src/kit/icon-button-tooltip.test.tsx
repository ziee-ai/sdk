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

/**
 * The regression the tooltip fix INTRODUCED, and the pass that caught it.
 *
 * Once composing a tooltip onto a trigger worked, the natural spelling for a raw trigger became
 * `<Popover><Tooltip><button/></Tooltip></Popover>`. But `Popover`/`Dropdown` guess Base UI's
 * `nativeButton` flag from the child's TYPE, and a kit `<Tooltip>` is TRANSPARENT — it renders its
 * child through a Slot. So the guess flipped from "native button" to "not one", and Base UI logged
 * an ERROR on every render.
 *
 * Nothing else saw it: typecheck, lints, 2232 unit tests, the 94-surface tooltip sweep and 41 e2e
 * tests were all green on the tree that logged it. It was CytoAnalyst's runtime-health pass —
 * 6 gating HIGH findings on each of 5 workbench surfaces — that said so. Hence this test, which
 * makes the same statement in milliseconds.
 */
describe('E — a tooltip wrapper does not lie to Base UI about the tag underneath', () => {
  /**
   * THE ORACLE IS THE DOM, not the console. Base UI does log an error for this
   * ("…expected a non-<button> because the `nativeButton` prop is false…"), but it dedupes the
   * message process-wide, so a spy in one test is silent once any earlier test has tripped it —
   * a guard that passes for the wrong reason. The rendered difference does not dedupe:
   *
   *     nativeButton=true   … aria-haspopup=dialog | tabindex=0 | type=button
   *     nativeButton=false  … aria-haspopup=dialog | role=button | tabindex=0 | type=button
   *
   * `role="button"` on an element that already IS a button is Base UI supplying semantics the tag
   * already carries — i.e. the exact statement "I was told this is not a button", made in the DOM.
   */
  const roleOf = (id: string) => byTestId(id).getAttribute('role')

  it('a raw <button> inside a <Tooltip> inside a Popover trigger', () => {
    render(
      <Popover content={<div>body</div>}>
        <Tooltip content="Add a pane">
          <button type="button" aria-label="Add a pane" data-testid="e1">
            <Icon />
          </button>
        </Tooltip>
      </Popover>,
    )
    expect(
      roleOf('e1'),
      'the trigger IS a <button>; a redundant role means Base UI was told it was not, and it ' +
        'logs an error on every render saying so',
    ).toBeNull()
  })

  it('a kit <Button> inside a <Tooltip> inside a Dropdown trigger', () => {
    render(
      <Dropdown data-testid="e-dd" items={[{ key: 'a', label: 'A' }]}>
        <Tooltip content="More">
          <Button size="icon" aria-label="More" data-testid="e2">
            <Icon />
          </Button>
        </Tooltip>
      </Dropdown>,
    )
    expect(roleOf('e2')).toBeNull()
  })

  it('a NON-button child still gets the button semantics supplied — transparent, not a rubber stamp', () => {
    render(
      <Dropdown data-testid="e-dd2" items={[{ key: 'a', label: 'A' }]}>
        <Tooltip content="Project actions">
          <div tabIndex={0} aria-label="Project actions" data-testid="e3">
            <Icon />
          </div>
        </Tooltip>
      </Dropdown>,
    )
    expect(
      roleOf('e3'),
      'a <div> trigger needs the role Base UI supplies — resolving THROUGH the wrapper must not ' +
        'become "everything is a button"',
    ).toBe('button')
  })

  it('a COMPONENT child that renders a non-button is still not a button', () => {
    // The kit Button is the one component known to render a native <button>; every other
    // component has to be assumed otherwise, because a component cannot be introspected for the
    // tag it renders. `Tag` renders a <span> and is the case the Dropdown doc comment names.
    render(
      <Dropdown data-testid="e-dd3" items={[{ key: 'a', label: 'A' }]}>
        <Tag data-testid="e7">Filter</Tag>
      </Dropdown>,
    )
    expect(
      roleOf('e7'),
      'a <span> trigger needs the role Base UI supplies — "it is a component, so call it a ' +
        'button" would silently strip the semantics off every pill trigger in the kit',
    ).toBe('button')
  })

  it('the un-wrapped forms still resolve the way they always did', () => {
    render(
      <Popover content={<div>b</div>}>
        <button type="button" aria-label="Bare" data-testid="e4">
          <Icon />
        </button>
      </Popover>,
    )
    expect(roleOf('e4')).toBeNull()
    cleanup()
    render(
      <Popover content={<div>b</div>}>
        <Button size="icon" tooltip="Kit" icon={<Icon />} data-testid="e5" />
      </Popover>,
    )
    expect(roleOf('e5')).toBeNull()
    cleanup()
    render(
      <Popover content={<div>b</div>}>
        <div tabIndex={0} aria-label="Div" data-testid="e6">
          <Icon />
        </div>
      </Popover>,
    )
    expect(roleOf('e6'), 'a bare div trigger is still not a button').toBe('button')
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
  // These four assert the DIAGNOSTIC, so a console spy is the right oracle here (it IS the
  // subject). Note the deliberately distinct `data-testid` per case: `inertTooltipsWarned`
  // dedupes on `testid::label` for the life of the module, so two cases sharing an id would
  // make the second one pass because the FIRST already consumed the message. That is the same
  // process-wide-dedupe trap that made a console-spy oracle wrong for the BEHAVIOUR tests
  // below — there the subject is what a user sees, and it is asserted on the DOM.
  it('says so, naming the control and the remedy', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Button size="icon" tooltip="No more analyses to add" icon={<Icon />} disabled data-testid="c1" />)
    const said = err.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    expect(said, 'the message must name the control it is about').toContain('c1')
    expect(said, 'and it must name the remedy, or it is just noise').toMatch(/unavailableReason/)
  })

  // The escalation itself, pinned. A `warn` is advisory and scrolled past while 13 live sites
  // shipped an unreadable sentence; the consumer's only console-reading gate keys on ERROR, so
  // downgrading this silently would re-open the whole class with every other test still green.
  it('reports at ERROR level, because that is what the consumer gate reads', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    render(<Button size="icon" tooltip="Unreachable" icon={<Icon />} disabled data-testid="c1b" />)
    const atError = err.mock.calls.map(c => c.join(' ')).join('\n')
    const atWarn = warn.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    warn.mockRestore()
    expect(atError, 'a gate that reads console.error must be able to see this').toContain('c1b')
    expect(atWarn, 'and it must not ALSO be a warn, or the gate double-counts it').not.toContain('c1b')
  })

  it('says nothing for an ENABLED tooltipped button', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Button size="icon" tooltip="Fine" icon={<Icon />} data-testid="c2" />)
    const said = err.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    expect(said, 'a diagnostic that fires on the healthy case is one nobody reads').not.toContain('c2')
  })

  it('says nothing for a disabled button with NO tooltip', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <Button disabled data-testid="c3">
        Save
      </Button>,
    )
    const said = err.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    expect(said).not.toContain('c3')
  })

  it('says nothing for `loading`, which is aria-disabled and CAN open its tooltip', async () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(<Button size="icon" tooltip="Saving" icon={<Icon />} loading data-testid="c4" />)
    const said = err.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    expect(said, 'loading keeps the button focusable and hoverable on purpose').not.toContain('c4')
    expect(await tooltipOf(byTestId('c4'))).toBe('Saving')
  })
})

/**
 * D — `unavailableReason`: the remedy the message above names, as a prop rather than as an
 * instruction a caller has to re-implement.
 *
 * WHY IT IS A KIT PROP AND NOT A DOCUMENTED RECIPE. The recipe is five coupled parts —
 * `aria-disabled`, a dropped click handler, the disabled LOOK (which `disabled:opacity-50` no
 * longer supplies once the attribute is gone), the tooltip, and an `aria-describedby` pointing at
 * an sr-only node whose `id` must be UNIQUE IN THE DOCUMENT. The consumer hand-rolled it once and
 * needed a paragraph of commentary to get the id right; thirteen more call sites would each have
 * had to get all five right, and getting the id wrong fails SILENTLY (the browser resolves the
 * reference to whichever node came first, so one row announces another row's reason).
 *
 * Every assertion here reads the DOM. A console spy would be the wrong oracle twice over: the
 * subject is what a user gets, and base-ui/the kit dedupe messages process-wide, so a spy can
 * pass because an earlier test in the file already consumed the message.
 */
describe('D — `unavailableReason` makes the reachable form the easy form', () => {
  it('opens the REASON on hover, which native `disabled` cannot do at all', async () => {
    render(
      <Button
        size="icon"
        tooltip="Undo"
        icon={<Icon />}
        unavailableReason="Nothing to undo"
        data-testid="d1"
      />,
    )
    expect(await tooltipOf(byTestId('d1'))).toBe('Nothing to undo')
  })

  it('keeps the NAME saying what the control IS, not why it refused', () => {
    render(
      <Button
        size="icon"
        tooltip="Undo"
        icon={<Icon />}
        unavailableReason="Nothing to undo"
        data-testid="d2"
      />,
    )
    // The 93167985 rule: a reason that swallows the name leaves a screen-reader user unable to
    // tell WHICH control refused them.
    expect(accessibleName(byTestId('d2'))).toBe('Undo')
  })

  it('an explicit aria-label still wins the name', () => {
    render(
      <Button
        size="icon"
        aria-label="Close pane"
        icon={<Icon />}
        unavailableReason="This is the last open pane"
        data-testid="d3"
      />,
    )
    expect(byTestId('d3').getAttribute('aria-label')).toBe('Close pane')
  })

  it('puts the reason in the accessibility tree too, via aria-describedby', () => {
    render(
      <Button size="icon" tooltip="Undo" icon={<Icon />} unavailableReason="Nothing to undo" data-testid="d4" />,
    )
    const el = byTestId('d4')
    const ids = (el.getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean)
    expect(ids.length, 'an unavailable control must point at its reason').toBeGreaterThan(0)
    const text = ids.map(i => document.getElementById(i)?.textContent ?? '').join(' ').trim()
    expect(text).toBe('Nothing to undo')
  })

  it("does not discard a caller's OWN aria-describedby", () => {
    render(
      <>
        <span id="d5-extra">extra context</span>
        <Button
          size="icon"
          tooltip="Undo"
          icon={<Icon />}
          aria-describedby="d5-extra"
          unavailableReason="Nothing to undo"
          data-testid="d5"
        />
      </>,
    )
    const ids = (byTestId('d5').getAttribute('aria-describedby') ?? '').split(/\s+/).filter(Boolean)
    const text = ids.map(i => document.getElementById(i)?.textContent ?? '').join(' ')
    expect(text).toContain('extra context')
    expect(text).toContain('Nothing to undo')
  })

  it('gives each instance its OWN reason node — a duplicate id would cross the wires', () => {
    render(
      <>
        <Button size="icon" tooltip="Remove" icon={<Icon />} unavailableReason="Row A is locked" data-testid="d6a" />
        <Button size="icon" tooltip="Remove" icon={<Icon />} unavailableReason="Row B is locked" data-testid="d6b" />
      </>,
    )
    const read = (t: string) =>
      (byTestId(t).getAttribute('aria-describedby') ?? '')
        .split(/\s+/)
        .filter(Boolean)
        .map(i => document.getElementById(i)?.textContent ?? '')
        .join(' ')
        .trim()
    // The failure this pins is not "the ids are equal" but its CONSEQUENCE: `getElementById`
    // returns the first match, so both rows would announce row A's reason.
    expect(read('d6a')).toBe('Row A is locked')
    expect(read('d6b')).toBe('Row B is locked')
  })

  it('is NOT natively disabled — which is what makes every channel above reachable', () => {
    render(
      <Button size="icon" tooltip="Undo" icon={<Icon />} unavailableReason="Nothing to undo" data-testid="d7" />,
    )
    const el = byTestId('d7')
    expect(el.hasAttribute('disabled'), 'native disabled is pointer-events:none + no tab stop').toBe(false)
    expect(el.getAttribute('aria-disabled')).toBe('true')
  })

  it('still refuses activation — `aria-disabled` is a claim, not an enforcement', () => {
    let clicks = 0
    render(
      <Button
        size="icon"
        tooltip="Undo"
        icon={<Icon />}
        unavailableReason="Nothing to undo"
        onClick={() => {
          clicks += 1
        }}
        data-testid="d8"
      />,
    )
    fireEvent.click(byTestId('d8'))
    expect(clicks, 'an unavailable control that still fires is worse than a disabled one').toBe(0)
  })

  it('LOOKS unavailable — `disabled:opacity-50` no longer applies without the attribute', () => {
    render(
      <Button size="icon" tooltip="Undo" icon={<Icon />} unavailableReason="Nothing to undo" data-testid="d9" />,
    )
    const cls = byTestId('d9').getAttribute('class') ?? ''
    expect(cls, 'a control that reads as live but refuses every click is a broken button').toContain('opacity-50')
    expect(cls).toContain('cursor-not-allowed')
  })

  it('an EMPTY reason is not a reason — the control stays fully live', async () => {
    render(<Button size="icon" tooltip="Undo" icon={<Icon />} unavailableReason="" data-testid="d10" />)
    const el = byTestId('d10')
    expect(el.getAttribute('aria-disabled')).toBeNull()
    expect(await tooltipOf(el)).toBe('Undo')
  })

  it('emits no inert-tooltip diagnostic, because there is nothing inert about it', () => {
    const err = vi.spyOn(console, 'error').mockImplementation(() => {})
    render(
      <Button size="icon" tooltip="Undo" icon={<Icon />} unavailableReason="Nothing to undo" data-testid="d11" />,
    )
    const said = err.mock.calls.map(c => c.join(' ')).join('\n')
    err.mockRestore()
    expect(said).not.toContain('d11')
  })

  it('a reason OVERRIDES a native disable rather than losing to it', async () => {
    // A caller migrating a site may leave `disabled` in place. Silently honoring it would make
    // the reason unreachable again — with the caller believing they had fixed it.
    render(
      <Button
        size="icon"
        tooltip="Undo"
        icon={<Icon />}
        disabled
        unavailableReason="Nothing to undo"
        data-testid="d12"
      />,
    )
    expect(byTestId('d12').hasAttribute('disabled')).toBe(false)
    expect(await tooltipOf(byTestId('d12'))).toBe('Nothing to undo')
  })
})
