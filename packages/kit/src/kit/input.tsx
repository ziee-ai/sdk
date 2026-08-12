import * as React from 'react'
import { Eye, EyeOff, Loader2, X } from 'lucide-react'
import { Input as InputBase } from '../shadcn/input'
import { Skeleton } from '../shadcn/skeleton'
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'
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

export const Input = React.forwardRef<HTMLInputElement, InputProps>((allProps, ref) => {
  const { size: ownSize, loading, prefix, suffix, invalid, disabled, readOnly, allowClear, style, allowStyle, className, ...props } = allProps
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

  // WHETHER THERE IS A WRAPPER IS DECIDED BY THE CALL SITE, NEVER BY THE VALUE.
  //
  // This was `!!(prefix || rightAdornment)` — and `rightAdornment` is VALUE-DERIVED: the
  // `allowClear` × exists only while the field is non-empty. So this component's ROOT element
  // changed from <input> to <div> on the FIRST character typed into an `allowClear` field (and
  // back again when it was cleared). React reconciles a changed root type by unmounting the old
  // subtree and mounting a new one, so the input the user was typing into was DESTROYED and
  // replaced mid-keystroke — taking focus, caret position, selection and IME composition with
  // it. The user typed one letter and had to re-click to type the second.
  //
  // Deriving it from which adornment SLOTS the caller declared gives the field ONE element tree
  // for its whole life: filling or emptying a slot then changes this element's children and
  // classes, which React updates in place. Key PRESENCE, not truthiness, so a call site whose
  // adornment is itself conditional (`suffix={busy ? <Spin/> : undefined}`) declares the slot on
  // every render and stays structurally stable too.
  //
  // Do NOT "fix" a focus loss here by re-focusing in an effect: that re-focuses a DIFFERENT
  // element, steals focus back from wherever the user moved on to, and still discards the caret.
  const adorned =
    'prefix' in allProps || 'suffix' in allProps || 'allowClear' in allProps || 'loading' in allProps

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
})
Input.displayName = 'Input'

// shadcn has no password input — kit addition with a keyboard-accessible show/hide toggle.
// showLabel/hideLabel are REQUIRED (no default) so the toggle's accessible name is always
// caller-owned and translatable.
// `maxLength` IS OMITTED, AND THAT IS THE POINT.
//
// `maxlength` enforces a cap by silently DISCARDING the overflow — the third case in
// `internal/clamp-notice.tsx` ("characters past a cap"), which that rule says must announce what
// was kept and what was dropped. On a MASKED field neither response works. The user cannot read
// what survived, so a truncation is undetectable; and announcing "kept 72, dropped 18" would be
// announcing that the app chose a credential the user did not.
//
// The failure it produced is not theoretical: cytoanalyst's profile page capped BOTH the new and
// the confirm password at 72, so a longer passphrase was cut identically in both, the two matched,
// validation passed, the change succeeded — and the passphrase the user saved was not the one that
// had been set. There is no signal anywhere in that sequence that anything was dropped.
//
// So the cap is not expressible here. A password length limit is a REFUSAL that states its limit
// (see the app's `core/passwordPolicy`), never a rewrite. Note this omission covers `PasswordInput`
// only — `<Input type="password">` is the same hazard through a different door, which is why the
// app also lints for it.
export type PasswordInputProps = Omit<
  InputProps,
  'type' | 'suffix' | 'style' | 'allowStyle' | 'maxLength'
> & {
  showLabel: string
  hideLabel: string
}
export const PasswordInput = React.forwardRef<HTMLInputElement, PasswordInputProps>(
  ({ showLabel, hideLabel, ...rest }, ref) => {
    const [show, setShow] = React.useState(false)
    // The type omission above stops the honest caller; this stops the one who reached the prop
    // anyway — a spread of a wider props object, an `as never`, or plain JS. A cap that arrives
    // here is dropped rather than forwarded, so no path through this component can truncate a
    // secret. Deleting this line is invisible to the type system, so a test drives it.
    const { maxLength: _bannedCap, ...props } = rest as typeof rest & { maxLength?: number }
    return (
      <Input
        {...props}
        ref={ref}
        type={show ? 'text' : 'password'}
        suffix={
          /* The reveal toggle is an eye glyph and nothing else, and which of the two things it
             means depends on the CURRENT state — so the label has to be visible on hover, not
             only in the accessibility tree. Both channels are fed the same state-dependent
             string, so they flip together and can never disagree. */
          <TooltipProvider delay={300}>
            <Tooltip>
              <TooltipTrigger
                render={
                  <button
                    type="button"
                    data-slot="password-reveal"
                    onClick={() => setShow((v) => !v)}
                    className="pointer-events-auto text-muted-foreground hover:text-foreground"
                    aria-label={show ? hideLabel : showLabel}
                    aria-pressed={show}
                  >
                    {show ? <EyeOff aria-hidden /> : <Eye aria-hidden />}
                  </button>
                }
              />
              <TooltipContent>{show ? hideLabel : showLabel}</TooltipContent>
            </Tooltip>
          </TooltipProvider>
        }
      />
    )
  },
)
PasswordInput.displayName = 'PasswordInput'
