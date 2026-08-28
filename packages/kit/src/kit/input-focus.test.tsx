// @vitest-environment jsdom
//
// The kit `Input` must not swap its own element out from under the person typing into it.
//
// The reported defect: the FIRST character typed into an `allowClear` field dropped focus, and
// clearing it back to empty dropped focus again — the user typed one letter and had to re-click.
// Both are the same boundary (`value === ''` ⇄ `value !== ''`), because this component's ROOT
// element used to be decided by whether an adornment was CURRENTLY SHOWING: with nothing to show
// it returned the bare <input>, and the moment the × appeared it returned <div><input/>…</div>.
// React reconciles a changed root type by unmounting the old subtree — so the field the user was
// typing into was destroyed and rebuilt between keystrokes.
//
// The invariant these tests pin is therefore about ELEMENT IDENTITY, not about focus being
// restored: the same DOM node must still be the document's `activeElement`. A component that
// re-focused a NEW node in an effect would satisfy "is focused" while still throwing away the
// caret, the selection and any IME composition — and would steal focus back from wherever the
// user had moved on to. That is the fix this file is written to reject.
//
// Every focus assertion is made after keystroke ONE. Typing a whole word and then asserting
// focus passes VACUOUSLY against this bug: only the ''→'c' transition swaps the tree.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { Input } from './input'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
})

afterEach(cleanup)

/** A controlled `allowClear` field — the shape every search box in the app uses. */
function ClearableField({ initial = '' }: { initial?: string }) {
  const [value, setValue] = React.useState(initial)
  return (
    <Input
      allowClear
      aria-label="Search"
      value={value}
      onChange={e => setValue(e.target.value)}
      data-testid="field"
    />
  )
}

const field = (): HTMLInputElement => {
  const el = document.querySelector<HTMLInputElement>('[data-testid="field"]')
  if (!el) throw new Error('the field is not on screen')
  return el
}

describe('Input keeps ONE element across the empty ⇄ non-empty boundary', () => {
  it('survives the FIRST character typed into an allowClear field', () => {
    render(<ClearableField />)
    const input = field()
    input.focus()
    expect(document.activeElement, 'precondition: focused before the keystroke').toBe(input)

    fireEvent.change(input, { target: { value: 'c' } })

    expect(document.body.contains(input), 'the same node is still in the document').toBe(true)
    expect(document.activeElement, 'focus survives keystroke ONE').toBe(input)
    expect(field(), 'and it is still the field on screen — nothing was swapped in beside it').toBe(input)
    expect(input.value).toBe('c')
  })

  it('survives clearing the value back to empty', () => {
    render(<ClearableField initial="cluster" />)
    const input = field()
    input.focus()
    expect(document.activeElement).toBe(input)

    fireEvent.change(input, { target: { value: '' } })

    expect(document.body.contains(input), 'the same node is still in the document').toBe(true)
    expect(document.activeElement, 'focus survives the transition back to empty').toBe(input)
    expect(input.value).toBe('')
  })

  it('still shows a working clear button once there is a value', () => {
    // The structural fix must not cost the affordance it was tangled up with: the × appears
    // only with a value, clears it, and does so WITHOUT replacing the field.
    render(<ClearableField />)
    const input = field()
    expect(document.querySelector('[aria-label="Clear"]'), 'no × on an empty field').toBeNull()

    fireEvent.change(input, { target: { value: 'abc' } })
    const clear = document.querySelector<HTMLButtonElement>('[aria-label="Clear"]')
    expect(clear, 'the × appears once there is something to clear').toBeTruthy()

    fireEvent.click(clear!)
    expect(input.value, 'the × emptied the field').toBe('')
    expect(field(), 'and did it in place — the field itself was never replaced').toBe(input)
  })

  it('stays stable when the caller declares a slot it only sometimes fills', () => {
    // `suffix={busy ? <Spinner/> : undefined}` is the same trap one level up: the slot is
    // declared on every render even though it is empty on most of them, so the structure must
    // follow the DECLARATION, not the current content.
    function ConditionalSuffix() {
      const [value, setValue] = React.useState('')
      return (
        <Input
          aria-label="Search"
          value={value}
          onChange={e => setValue(e.target.value)}
          suffix={value ? <span data-testid="suffix">!</span> : undefined}
          data-testid="field"
        />
      )
    }
    render(<ConditionalSuffix />)
    const input = field()
    input.focus()

    fireEvent.change(input, { target: { value: 'x' } })

    expect(document.activeElement, 'a slot filling up is a child change, not a remount').toBe(input)
    expect(document.querySelector('[data-testid="suffix"]'), 'the suffix did render').toBeTruthy()
  })

  it('leaves a plain Input unwrapped, with the caller className on the field itself', () => {
    // The blast-radius guarantee, and the reason the wrapper is not simply made unconditional:
    // an Input that declares NO adornment slot renders exactly what it always did — the bare
    // <input> as its own root, carrying the caller's layout classes. Wrapping those too would
    // move `flex-1`/`w-32` onto a new div and silently re-layout every form in the app.
    const { container } = render(<Input aria-label="Plain" className="w-32" data-testid="plain" />)
    const el = container.querySelector<HTMLInputElement>('[data-testid="plain"]')
    expect(el, 'the field rendered').toBeTruthy()
    expect(container.firstElementChild, 'the input IS the root — no wrapper was added').toBe(el)
    expect(el!.className, "the caller's className is on the input").toContain('w-32')
  })
})
