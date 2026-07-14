import * as React from 'react'
import { RadioGroup as Base, RadioGroupItem } from '../shadcn/radio-group'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { cn } from '../lib/utils'
import type { ValueBinding } from './value-binding'

export interface RadioOption {
  label: React.ReactNode
  value: string
  disabled?: boolean
}

interface RadioGroupBase {
  options: RadioOption[]
  onBlur?: () => void
  disabled?: boolean
  name?: string
  id?: string
  invalid?: boolean
  orientation?: 'vertical' | 'horizontal'
  className?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-describedby'?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type RadioGroupProps = RadioGroupBase & ValueBinding<string>

export const RadioGroup = React.forwardRef<HTMLDivElement, RadioGroupProps>(function RadioGroup(
  { options, value, defaultValue, onValueChange, onChange, onBlur, disabled, name, id, invalid, orientation = 'vertical', className,
    'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby, 'aria-describedby': ariaDescribedby,
    'data-testid': testid },
  ref,
) {
  const s = useSurface({ disabled })
  const uid = React.useId()
  const handle = (v: string) => {
    onValueChange?.(v)
    onChange?.(v)
  }
  if (s.loading) {
    return (
      <div className={cn('grid gap-2', className)}>
        {options.map((o) => <Skeleton key={o.value} className="h-5 w-32 rounded" />)}
      </div>
    )
  }
  return (
    <Base
      ref={ref}
      id={id}
      value={value}
      defaultValue={defaultValue}
      onValueChange={handle}
      onBlur={onBlur}
      disabled={s.disabled || s.readOnly}
      name={name}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledby}
      aria-describedby={ariaDescribedby}
      aria-invalid={invalid || undefined}
      data-testid={testid}
      className={cn(orientation === 'horizontal' ? 'flex flex-wrap gap-4' : 'grid gap-2', className)}
    >
      {options.map((o) => {
        const itemId = `${uid}-${o.value}`
        return (
          <div key={o.value} className="flex items-center gap-2">
            <RadioGroupItem value={o.value} id={itemId} disabled={o.disabled} />
            <label htmlFor={itemId} className={cn('text-sm', (o.disabled || s.disabled) && 'opacity-60')}>{o.label}</label>
          </div>
        )
      })}
    </Base>
  )
})
