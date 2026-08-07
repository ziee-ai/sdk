// @vitest-environment jsdom
//
// The ONE property of `Slider` a consuming app cannot fix from outside: the accessible NAME of
// the `<input type="range">` each thumb wraps.
//
// A caller's `aria-label` lands on the ROOT `<div>`, and the thing a keyboard or screen-reader
// user actually operates is the nested input — which had no name at all. axe reports it as a
// CRITICAL violation, and it is not a false one. It survived because no gallery surface rendered
// a slider until a per-row range control put one on screen.
//
// The two assertions are the two halves of the fix being real rather than intended: a range
// slider's two inputs are NAMED, and they are named DIFFERENTLY (two operable controls sharing
// one name are indistinguishable to anyone not looking at the screen). The third pins the
// non-invention rule: with no `aria-label` on the root the kit adds none, because a fabricated
// label would override an `aria-labelledby` the caller supplied instead.

import { describe, it, expect, afterEach } from 'vitest'
import * as React from 'react'
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { Slider } from '../shadcn/slider'

let host: HTMLDivElement | null = null
let root: Root | null = null

function render(node: React.ReactElement): HTMLDivElement {
  host = document.createElement('div')
  document.body.appendChild(host)
  root = createRoot(host)
  act(() => {
    root!.render(node)
  })
  return host
}

afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  host = null
  root = null
})

const rangeInputs = (el: HTMLElement): HTMLInputElement[] =>
  Array.from(el.querySelectorAll('input[type="range"]'))

describe('Slider thumb accessible names', () => {
  it('names every thumb input of a RANGE slider, and names them apart', () => {
    const el = render(<Slider min={0} max={100} value={[10, 90]} aria-label="threshold" />)
    const inputs = rangeInputs(el)
    expect(inputs).toHaveLength(2)
    const names = inputs.map(i => i.getAttribute('aria-label'))
    for (const n of names) expect(n, 'a thumb input rendered with no accessible name').toBeTruthy()
    expect(new Set(names).size, `both thumbs answer to the same name: ${names.join(' / ')}`).toBe(2)
    for (const n of names) expect(n).toContain('threshold')
  })

  it('names a SINGLE-thumb slider with the label verbatim (nothing to disambiguate)', () => {
    const el = render(<Slider min={0} max={100} value={[50]} aria-label="opacity" />)
    const inputs = rangeInputs(el)
    expect(inputs).toHaveLength(1)
    expect(inputs[0].getAttribute('aria-label')).toBe('opacity')
  })

  it('invents NO name when the root carries none', () => {
    const el = render(<Slider min={0} max={100} value={[10, 90]} />)
    for (const input of rangeInputs(el)) {
      expect(input.getAttribute('aria-label')).toBeNull()
    }
  })
})
