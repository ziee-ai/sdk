import * as React from 'react'
import { Checkbox as Base } from '../shadcn/checkbox'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { cn } from '../lib/utils'
import type { CheckedBinding } from './value-binding'

interface CheckboxBase {
  /** Mixed state (legacy `indeterminate`); overrides `checked` visually until toggled. */
  indeterminate?: boolean
  onBlur?: () => void
  disabled?: boolean
  name?: string
  id?: string
  label?: React.ReactNode
  className?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-describedby'?: string
  invalid?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// Controlled `checked` requires a change handler (see CheckedBinding); FormField stays valid.
export type CheckboxProps = CheckboxBase & CheckedBinding

export const Checkbox = React.forwardRef<HTMLButtonElement, CheckboxProps>(function Checkbox(
  { checked, value, defaultChecked, indeterminate, onCheckedChange, onChange, onBlur, disabled, name, id, label, className,
    'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby, 'aria-describedby': ariaDescribedby, invalid,
    'data-testid': testid },
  ref,
) {
  const s = useSurface({ disabled })
  const reactId = React.useId()
  const ctrlId = id ?? reactId
  const handle = (v: boolean | 'indeterminate') => {
    const b = v === true
    onCheckedChange?.(b)
    onChange?.(b)
  }
  if (s.loading) return <Skeleton className={cn('size-4 rounded', className)} />
  const control = (
    <Base
      ref={ref}
      id={ctrlId}
      checked={checked ?? value}
      indeterminate={indeterminate}
      defaultChecked={defaultChecked}
      onCheckedChange={handle}
      onBlur={onBlur}
      disabled={s.disabled || s.readOnly}
      name={name}
      aria-label={label == null ? ariaLabel : undefined}
      aria-labelledby={ariaLabelledby}
      aria-describedby={ariaDescribedby}
      aria-invalid={invalid || undefined}
      data-testid={testid}
      className={className}
    />
  )
  if (label == null) return control
  // sibling label (NOT nested) — nesting + htmlFor double-fires the toggle.
  return (
    <div className="flex items-center gap-2">
      {control}
      <label htmlFor={ctrlId} className={cn('text-sm', s.disabled && 'opacity-60')}>{label}</label>
    </div>
  )
})
