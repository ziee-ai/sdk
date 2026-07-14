import * as React from 'react'
import { Input, type InputProps } from './input'

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
    setBuf(raw)
    if (raw === '' || isIntermediate(raw)) {
      onChange?.(undefined)
      return
    }
    const n = Number(raw)
    if (!Number.isNaN(n)) onChange?.(n)
  }

  const handleBlur = () => {
    // clamp to range on blur, then normalize the displayed string.
    let n = buf === '' ? undefined : Number(buf)
    if (n !== undefined && !Number.isNaN(n)) {
      if (min !== undefined && n < min) n = min
      if (max !== undefined && n > max) n = max
      if (precision !== undefined) n = Number(n.toFixed(precision))
      setBuf(String(n))
      onChange?.(n)
    } else if (Number.isNaN(n as number)) {
      setBuf(numToStr(value))
    }
    onBlur?.()
  }

  return (
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
  )
})
