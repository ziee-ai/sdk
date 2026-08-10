// @vitest-environment jsdom
//
// A kit control's NAMED prop is the only way to set the state it owns — passing the bare
// `aria-*` attribute instead must not compile.
//
// The defect this pins: `ClosedSetSelect` passed `aria-invalid` to the kit `Select`, whose prop
// for that state is `invalid`. `Select` destructures a fixed prop list and spreads no rest, so the
// attribute was dropped; every closed set in a declarative form rendered its validation message
// with no programmatic invalid state and no invalid styling. It type-checked because JSX exempts
// HYPHENATED attribute names from excess-property checking (see `aria-passthrough.ts` — it is not
// the union, which is the intuitive but wrong diagnosis).
//
// ── BOTH HALVES, DELIBERATELY ─────────────────────────────────────────────────
//
// The type half (`@ts-expect-error`) is the gate: each directive FAILS THE BUILD if the line below
// it stops being an error, so removing the ban turns `tsc --noEmit` red rather than quietly
// reopening the hole. It is compiled by the kit's own `tsc --noEmit`, which the `test` script now
// runs, so it is gated by `just check-packages` alongside this suite.
//
// The runtime half is why the type half has to exist at all: it MEASURES the drop. Nothing here
// makes `Select` forward `aria-invalid` — it still does not, and it should not, because `invalid`
// is the prop. The point is that the drop is silent, so the type system is the only thing that can
// report it, and a test that asserted only the type would not have established that there was
// anything to report.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { cleanup, render } from '@testing-library/react'
import { Select } from './select'
import { Switch } from './switch'
import { Checkbox } from './checkbox'
import { RadioGroup } from './radio-group'
import { Combobox } from './combobox'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
})

afterEach(cleanup)

const OPTIONS = [
  { label: 'One', value: 'one' },
  { label: 'Two', value: 'two' },
]

const trigger = (testid: string): HTMLElement => {
  const el = document.querySelector<HTMLElement>(`[data-testid="${testid}"]`)
  if (!el) throw new Error(`the control did not render: ${testid}`)
  return el
}

describe('a kit control derives aria-* from its named prop', () => {
  it('`invalid` reaches the DOM as aria-invalid', () => {
    render(<Select data-testid="s-named" options={OPTIONS} invalid aria-label="Pick" />)
    expect(trigger('s-named').getAttribute('aria-invalid')).toBe('true')
  })

  it('a bare `aria-invalid` is DROPPED — the measurement the ban exists for', () => {
    render(
      // The directive sits on the ELEMENT, not on the attribute: an incompatible JSX attribute is
      // reported at the opening tag, so a directive beside the attribute would be "unused" and the
      // real error would escape. Same reason the bans below are written one element per line.
      // @ts-expect-error `aria-invalid` is not forwarded by this control — pass `invalid`.
      <Select data-testid="s-attr" options={OPTIONS} aria-label="Pick" aria-invalid />,
    )
    expect(
      trigger('s-attr').getAttribute('aria-invalid'),
      'if this ever becomes "true" the control started forwarding the attribute and the ban ' +
        'below should be reconsidered — but it must not be BOTH dropped and legal',
    ).toBe(null)
  })

  it('the same holds for a control that is not a Select', () => {
    render(<Switch data-testid="w-named" invalid aria-label="Toggle" />)
    expect(trigger('w-named').getAttribute('aria-invalid')).toBe('true')
  })
})

// ── THE OTHER HALF OF THE SAME DROP: what `FormField` INJECTS ─────────────────
//
// `FormField required` sets `aria-required` on its child through `cloneElement` with an untyped
// `Record<string, unknown>`. That injection is invisible to the ban above — it is not a JSX
// attribute and it is not type-checked — so a control that neither declared nor forwarded
// `aria-required` dropped it exactly as `Select` dropped `aria-invalid`, and no gate could say so.
// Found while deriving the ban: Checkbox, Switch, RadioGroup and Combobox were all in that state,
// so `<FormField required>` around any of them announced nothing.
//
// Segmented is NOT here and that is deliberate: it renders a `tablist`, and `aria-required` is not
// a permitted attribute of that role. It stays banned, and `FormField required` around a Segmented
// is meaningless rather than merely dropped.

describe('aria-required survives the FormField injection path', () => {
  it.each([
    ['checkbox', <Checkbox key="c" data-testid="r-checkbox" aria-label="C" aria-required />],
    ['switch', <Switch key="s" data-testid="r-switch" aria-label="S" aria-required />],
    ['radio-group', <RadioGroup key="r" data-testid="r-radio" aria-label="R" options={[{ label: 'a', value: 'a' }]} aria-required />],
    ['combobox', <Combobox key="b" data-testid="r-combobox" aria-label="B" placeholder="p" emptyText="e" options={[]} aria-required />],
  ])('%s forwards it to the DOM', (_name, el) => {
    render(el)
    const id = (el as React.ReactElement<{ 'data-testid': string }>).props['data-testid']
    expect(trigger(id).getAttribute('aria-required')).toBe('true')
  })
})

// ── THE TYPE HALF ─────────────────────────────────────────────────────────────
//
// Each `@ts-expect-error` is an assertion in its own right: tsc reports an UNUSED directive as an
// error, so a line that stops failing turns the build red. They are grouped in one unrendered
// component because JSX is the only position in which the hyphenated-attribute rule applies —
// object-literal assignment to `SelectProps` catches these already.

export function __TypeBans(): React.ReactElement {
  return (
    <>
      {/* aria-invalid ← `invalid`, on every closed-prop control. */}
      {/* @ts-expect-error use `invalid` */}
      <Select data-testid="t1" options={OPTIONS} aria-label="x" aria-invalid />
      {/* @ts-expect-error use `invalid` */}
      <Switch data-testid="t2" aria-label="x" aria-invalid={true} />

      {/* aria-busy ← `loading`. */}
      {/* @ts-expect-error use `loading` */}
      <Select data-testid="t3" options={OPTIONS} aria-label="x" aria-busy />

      {/* aria-disabled ← `disabled`. */}
      {/* @ts-expect-error use `disabled` */}
      <Select data-testid="t4" options={OPTIONS} aria-label="x" aria-disabled />

      {/* An aria attribute the control simply does not forward, with no named prop behind it:
          still a silent drop, still banned. */}
      {/* @ts-expect-error not forwarded */}
      <Select data-testid="t5" options={OPTIONS} aria-label="x" aria-atomic />

      {/* THE NEGATIVE CONTROLS. The ban must not swallow what the control DOES declare — a ban
          that rejected these would be found the moment anyone labelled a control, but it would be
          found by them and not by this file. */}
      <Select
        data-testid="t6"
        options={OPTIONS}
        aria-label="x"
        aria-describedby="d"
        aria-required
        invalid
        loading
        disabled
      />
      <Select data-testid="t7" options={OPTIONS} aria-labelledby="l" />
      {/* …and `undefined` stays legal, so a conditional spread of an absent attribute compiles. */}
      <Select data-testid="t8" options={OPTIONS} aria-label="x" aria-invalid={undefined} />
    </>
  )
}
