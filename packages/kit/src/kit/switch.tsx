import * as React from 'react'
import { Loader2 } from 'lucide-react'
import { Switch as Base } from '../shadcn/switch'
import { Skeleton } from '../shadcn/skeleton'
import { Tooltip } from './tooltip'
import { useSurface } from './surface'
import { cn } from '../lib/utils'
import type { CheckedBinding } from './value-binding'
import type { NoUndeclaredAria } from './aria-passthrough'

interface SwitchBase {
  onBlur?: () => void
  disabled?: boolean
  /** Own busy state (legacy `loading`): disables + shows a spinner. NOT the region skeleton. */
  loading?: boolean
  size?: 'sm' | 'default'
  name?: string
  id?: string
  /** Inline label; if omitted, provide aria-label/aria-labelledby or wrap in a FormField. */
  label?: React.ReactNode
  /** Tooltip on hover/focus. For a label-less switch it ALSO becomes the accessible
   * name (aria-label) when it's a string — so a bare switch is never nameless. */
  tooltip?: React.ReactNode
  className?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-describedby'?: string
  /** Marks the control required for assistive tech. `FormField required` INJECTS this (via
   *  cloneElement, untyped), so a control that neither declares nor forwards it drops it
   *  silently — the same silent-drop class `aria-passthrough.ts` exists for, and one the type
   *  ban cannot see because the injection is untyped. */
  'aria-required'?: boolean
  invalid?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// Controlled `checked` requires a change handler (see CheckedBinding); FormField stays valid.
export type SwitchProps = SwitchBase & NoUndeclaredAria<SwitchBase> & CheckedBinding

export const Switch = React.forwardRef<HTMLButtonElement, SwitchProps>(function Switch(
  { checked, value, defaultChecked, onCheckedChange, onChange, onBlur, disabled, loading, size, name, id, label, tooltip, className,
    'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby, 'aria-describedby': ariaDescribedby, 'aria-required': ariaRequired, invalid,
    'data-testid': testid },
  ref,
) {
  // A label-less switch with a string tooltip gets that tooltip as its accessible name.
  const nameFromTooltip = typeof tooltip === 'string' ? tooltip : undefined
  const s = useSurface({ disabled })
  const reactId = React.useId()
  const ctrlId = id ?? reactId
  // compose: both the form binding (onChange) and any consumer canonical handler fire.
  const handle = (v: boolean) => {
    onCheckedChange?.(v)
    onChange?.(v)
  }
  if (s.loading) return <Skeleton className={cn('h-[1.15rem] w-8 rounded-full', className)} />
  // Controlled intent = a bound value or a change handler is present. In that case
  // ALWAYS pass a defined boolean (coerce a not-yet-loaded `undefined` → false) so
  // the switch never flips uncontrolled→controlled when the backing state resolves
  // (Base UI warns on that transition). Only a purely-uncontrolled switch
  // (defaultChecked, no value/handler) stays uncontrolled.
  const controlled =
    checked !== undefined || value !== undefined || onCheckedChange !== undefined || onChange !== undefined
  const baseEl = (
    <Base
      ref={ref}
      id={ctrlId}
      checked={controlled ? Boolean(checked ?? value) : undefined}
      defaultChecked={controlled ? undefined : defaultChecked}
      onCheckedChange={handle}
      onBlur={onBlur}
      disabled={s.disabled || s.readOnly || loading}
      name={name}
      aria-label={label == null ? (ariaLabel ?? nameFromTooltip) : undefined}
      aria-labelledby={ariaLabelledby}
      aria-describedby={ariaDescribedby}
      aria-invalid={invalid || undefined}
      aria-required={ariaRequired || undefined}
      aria-busy={loading || undefined}
      data-testid={testid}
      size={size}
      className={className}
    />
  )
  // own loading → spinner overlay on the track (not a skeleton).
  const control = loading ? (
    <span className="relative inline-flex">
      {baseEl}
      <Loader2 className="pointer-events-none absolute left-1/2 top-1/2 size-3 -translate-x-1/2 -translate-y-1/2 animate-spin opacity-80" aria-hidden />
    </span>
  ) : baseEl
  // Tooltip trigger is an inert wrapping <span>, NOT the switch itself — making
  // the tiny (~18px) interactive switch the trigger caused base-ui to open then
  // immediately close the tooltip (flicker). Same stable pattern as the kit
  // Tooltip used everywhere else.
  const maybeTip = tooltip != null ? (
    <Tooltip content={tooltip}>
      {/* w-fit: inside a FormField's `flex flex-col` (align-items: stretch) a
          width-less wrapper would stretch to the full row, so the tooltip (anchored
          to this span) would center mid-form instead of over the ~32px switch.
          w-fit alone constrains the width; do NOT add self-start — in a horizontal
          row (e.g. a table cell next to buttons) it would top-align the switch and
          break its vertical alignment with sibling buttons. */}
      <span className="inline-flex w-fit">{control}</span>
    </Tooltip>
  ) : control
  if (label == null) return maybeTip
  return (
    <div className="flex items-center gap-2">
      {control}
      <label htmlFor={ctrlId} className={cn('text-sm', s.disabled && 'opacity-60')}>{label}</label>
    </div>
  )
})
