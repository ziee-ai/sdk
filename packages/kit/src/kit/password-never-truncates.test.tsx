// @vitest-environment jsdom
//
// A MASKED FIELD MAY NOT CARRY A TRUNCATING CAP — THROUGH ANY DOOR.
//
// `maxlength` is the one constraint whose enforcement is a silent rewrite: the browser keeps a
// prefix and drops the rest with no event, no notice and no visible difference. On a masked input
// the user cannot see which happened, and when a confirm field carries the SAME cap the two
// truncated halves agree, so even the mismatch check goes quiet. The result is a credential the
// user believes they chose and can never reproduce.
//
// `PasswordInputProps` therefore omits `maxLength`, and the component drops it at runtime as well.
// The type omission is checked by the build; this file checks the runtime strip, because deleting
// that line is invisible to `tsc` and would silently restore the hazard for any caller that
// spreads a wider props object.
//
// The oracle is the RENDERED element's `maxLength` IDL attribute (−1 when absent), not the props
// object — that is the value the browser would actually enforce.

import { cleanup, render } from '@testing-library/react'
import { afterEach, describe, expect, it } from 'vitest'
import { Input, PasswordInput } from './input'

afterEach(cleanup)

const field = (id: string): HTMLInputElement => {
  const el = document.querySelector<HTMLInputElement>(`input[data-testid="${id}"]`)
  if (!el) throw new Error(`no input[data-testid="${id}"]`)
  return el
}

describe('PasswordInput', () => {
  it('declares no cap by default', () => {
    render(<PasswordInput data-testid="pw" showLabel="Show" hideLabel="Hide" />)
    expect(field('pw').maxLength).toBe(-1)
    expect(field('pw').hasAttribute('maxlength')).toBe(false)
  })

  it('drops a cap forced past the type system', () => {
    // What a caller spreading a wider props object does, expressed the only way the type system
    // still allows. If the runtime strip is removed this renders maxlength="72" and the field
    // silently truncates again.
    const forced = { maxLength: 72 } as unknown as Record<string, never>
    render(<PasswordInput data-testid="pw" showLabel="Show" hideLabel="Hide" {...forced} />)
    expect(field('pw').maxLength).toBe(-1)
    expect(field('pw').hasAttribute('maxlength')).toBe(false)
  })

  it('still forwards the props it is supposed to forward', () => {
    // The negative control: the strip must remove ONE prop, not act as a general filter. Without
    // this, replacing the destructure with `const props = {}` would also pass the two above.
    render(
      <PasswordInput
        data-testid="pw"
        showLabel="Show"
        hideLabel="Hide"
        placeholder="Enter a passphrase"
        autoComplete="new-password"
      />,
    )
    expect(field('pw').getAttribute('placeholder')).toBe('Enter a passphrase')
    expect(field('pw').getAttribute('autocomplete')).toBe('new-password')
    expect(field('pw').getAttribute('type')).toBe('password')
  })

  it('a plain Input still honours maxLength — the ban is specific to the masked field', () => {
    // The scope control. `maxLength` on a visible field is legitimate (the user can read what
    // survived), so a change that stripped it everywhere would be a different, wrong fix.
    render(<Input data-testid="plain" maxLength={255} />)
    expect(field('plain').maxLength).toBe(255)
  })
})
