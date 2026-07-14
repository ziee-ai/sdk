import * as React from 'react'
import { Eye, EyeOff, Loader2, X } from 'lucide-react'
import { Input as InputBase } from '../shadcn/input'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// Native <input> + kit additions (prefix/suffix adornments, invalid, density).
// `size` is density ('sm'|'default'|'lg'), NOT the native numeric size.
// Surface: region loading → skeleton · own `loading` → suffix spinner (+ disabled) · disabled · readOnly · size.
export type InputProps = Omit<React.ComponentProps<'input'>, 'size' | 'prefix' | 'style'> & {
  size?: 'sm' | 'default' | 'lg'
  loading?: boolean
  prefix?: React.ReactNode
  suffix?: React.ReactNode
  invalid?: boolean
  /** Show a clear (×) button when there's a value (legacy `allowClear`). Fires onChange with ''. */
  allowClear?: boolean
  /** Test selector — REQUIRED, forwarded onto the input via {...props} (i18n-safe). */
  'data-testid': string
} & KitStyleProps

export const Input = React.forwardRef<HTMLInputElement, InputProps>(
  ({ size: ownSize, loading, prefix, suffix, invalid, disabled, readOnly, allowClear, style, allowStyle, className, ...props }, ref) => {
    const s = useSurface({ disabled, readOnly, size: ownSize })

    if (s.loading) {
      return <Skeleton className={cn('h-9 w-full rounded-md', className)} />
    }

    const showClear = allowClear && props.value != null && props.value !== '' && !s.disabled && !s.readOnly && !loading
    const clearBtn = showClear ? (
      <button
        type="button"
        aria-label="Clear"
        className="pointer-events-auto text-muted-foreground hover:text-foreground"
        onClick={() => props.onChange?.({ target: { value: '' } } as React.ChangeEvent<HTMLInputElement>)}
      >
        <X className="size-4" aria-hidden />
      </button>
    ) : null
    const rightAdornment = loading ? <Loader2 className="size-4 animate-spin opacity-70" aria-hidden /> : (clearBtn ?? suffix)
    const adorned = !!(prefix || rightAdornment)
    const field = (
      <InputBase
        ref={ref}
        style={style}
        disabled={s.disabled || loading}
        readOnly={s.readOnly}
        aria-invalid={invalid || undefined}
        aria-busy={loading || undefined}
        className={cn(
          prefix && 'pl-9',
          rightAdornment && 'pr-9',
          // When adorned, sizing lives on the wrapper (below) so the adornment
          // sits at the input's edge; the field just fills that box.
          adorned ? 'w-full' : className,
        )}
        {...props}
        // A controlled input must never receive null/undefined (React warns + Base UI
        // flips uncontrolled↔controlled). Form bindings pass a `value` that may be null
        // before data loads → coerce to ''. Uncontrolled use (no `value`) is untouched.
        {...('value' in props ? { value: props.value ?? '' } : {})}
      />
    )
    if (!adorned) return field
    return (
      <div className={cn('relative w-full', className)}>
        {prefix && (
          <span className="pointer-events-none absolute left-3 top-1/2 -translate-y-1/2 text-muted-foreground [&_svg]:size-4">
            {prefix}
          </span>
        )}
        {field}
        {rightAdornment && (
          // non-interactive by default; interactive adornments (password toggle) opt back in.
          <span className="pointer-events-none absolute right-3 top-1/2 -translate-y-1/2 text-muted-foreground [&_svg]:size-4">
            {rightAdornment}
          </span>
        )}
      </div>
    )
  },
)
Input.displayName = 'Input'

// shadcn has no password input — kit addition with a keyboard-accessible show/hide toggle.
// showLabel/hideLabel are REQUIRED (no default) so the toggle's accessible name is always
// caller-owned and translatable.
export type PasswordInputProps = Omit<InputProps, 'type' | 'suffix' | 'style' | 'allowStyle'> & {
  showLabel: string
  hideLabel: string
}
export const PasswordInput = React.forwardRef<HTMLInputElement, PasswordInputProps>(
  ({ showLabel, hideLabel, ...props }, ref) => {
    const [show, setShow] = React.useState(false)
    return (
      <Input
        {...props}
        ref={ref}
        type={show ? 'text' : 'password'}
        suffix={
          <button
            type="button"
            onClick={() => setShow((v) => !v)}
            className="pointer-events-auto text-muted-foreground hover:text-foreground"
            aria-label={show ? hideLabel : showLabel}
            aria-pressed={show}
          >
            {show ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
          </button>
        }
      />
    )
  },
)
PasswordInput.displayName = 'PasswordInput'
