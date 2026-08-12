import * as React from 'react'
import { Input, type InputProps } from './input'
import { ClampNotice } from '../internal/clamp-notice'

// Numeric input. value/onChange speak `number` (not string). Empty/invalid → undefined.
// Keeps a local string buffer while editing so intermediate states ("1.", "-", "1.0")
// survive a controlled round-trip and never emit NaN. Clamps to min/max on blur.
export type InputNumberProps = Omit<InputProps, 'type' | 'value' | 'defaultValue' | 'onChange' | 'prefix' | 'style' | 'allowStyle'> & {
  // `null` is accepted (and shown as empty) so form fields whose "unset" value is
  // null — e.g. retention_days = null ("forever") — don't render the string "null".
  value?: number | null
  defaultValue?: number | null
  onChange?: (value: number | undefined) => void
  onBlur?: () => void
  min?: number
  max?: number
  step?: number
  /** Round the emitted/normalized value to N decimal places on blur (legacy `precision`). */
  precision?: number
  prefix?: React.ReactNode
}

const numToStr = (n: number | null | undefined) => (n == null || Number.isNaN(n) ? '' : String(n))
// A partial number the user may still be typing: "", "-", "1.", "1.0", "-0", "1e", "1e-".
const isIntermediate = (s: string) => s === '' || /^-?(\d*\.?\d*)?(e-?\d*)?$/i.test(s) && Number.isNaN(Number(s))

export const InputNumber = React.forwardRef<HTMLInputElement, InputNumberProps>(function InputNumber(
  { value, defaultValue, onChange, onBlur, min, max, step, precision, ...props }, ref,
) {
  const [buf, setBuf] = React.useState<string>(() => numToStr(value ?? defaultValue))
  // WHAT THE BLUR DID TO WHAT THE USER TYPED. Null while the two agree — the overwhelmingly
  // common case, and the one that must stay silent (see internal/clamp-notice.tsx rule 1).
  const [notice, setNotice] = React.useState<string | null>(null)
  // Sync the buffer from a controlled value only when it differs NUMERICALLY (so an
  // in-progress "1." isn't clobbered when the parent echoes back 1).
  React.useEffect(() => {
    if (value == null) {
      // controlled reset/clear (undefined OR null): empty the buffer so a null
      // "unset" value never renders as the literal string "null".
      if (buf !== '') setBuf('')
    } else if (Number(buf) !== value) {
      setBuf(numToStr(value))
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [value])

  const handleChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const raw = e.target.value
    // A notice describes what happened to a value that no longer exists the moment the user edits
    // again — leaving it up would be a message about nothing, beside a field that disagrees.
    setNotice(null)
    setBuf(raw)
    if (raw === '' || isIntermediate(raw)) {
      onChange?.(undefined)
      return
    }
    const n = Number(raw)
    if (!Number.isNaN(n)) onChange?.(n)
  }

  const handleBlur = () => {
    // Clamp to range on blur, then normalize the displayed string — AND SAY WHICH OF THOSE TWO
    // happened. The clamp itself was never the defect; doing it in silence was. The distinction
    // the notice encodes is the one the user cares about: 1.` → 1 is the same number spelled
    // differently (silent), 999 → 100 is not the number they asked for (announced).
    const typed = buf
    let n = typed === '' ? undefined : Number(typed)
    if (n !== undefined && !Number.isNaN(n)) {
      const parsed = n
      let why: string | null = null
      if (min !== undefined && n < min) { n = min; why = `Raised to the minimum, ${min}` }
      else if (max !== undefined && n > max) { n = max; why = `Lowered to the maximum, ${max}` }
      if (precision !== undefined) {
        const rounded = Number(n.toFixed(precision))
        if (rounded !== n) {
          n = rounded
          why = why ?? `Rounded to ${precision} decimal place${precision === 1 ? '' : 's'}: ${rounded}`
        } else {
          n = rounded
        }
      }
      // Belt and braces for any future step that changes the number without setting `why`: the
      // predicate is "the kept number differs from the parsed one", not "a branch remembered".
      if (why == null && n !== parsed) why = `Adjusted to ${n}`
      setBuf(String(n))
      setNotice(why)
      onChange?.(n)
    } else if (Number.isNaN(n as number)) {
      // The worst silent case in the family: the user's keystrokes vanish and the old number
      // reappears, which is indistinguishable from "my typing was not registered".
      const restored = numToStr(value)
      setBuf(restored)
      setNotice(
        restored === ''
          ? `“${typed}” is not a number — cleared`
          : `“${typed}” is not a number — kept ${restored}`,
      )
    } else {
      setNotice(null)
    }
    onBlur?.()
  }

  // `display:contents` so the notice becomes a sibling of the input in whatever layout the caller
  // already had (a `Field`'s column, a toolbar's row) instead of introducing a box of its own —
  // the same trick `Upload` uses to keep its file input out of the dropzone. The wrapper is
  // unconditional (never keyed off a value), so the input is never remounted mid-interaction —
  // the structural-stability rule input.tsx documents.
  return (
    <span className="contents">
      <Input
        ref={ref}
        type="text"
        inputMode="decimal"
        value={buf}
        min={min}
        max={max}
        step={step}
        onChange={handleChange}
        onBlur={handleBlur}
        {...props}
      />
      <ClampNotice message={notice} />
    </span>
  )
})
