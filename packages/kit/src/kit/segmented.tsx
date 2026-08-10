import * as React from 'react'
import { Tabs, TabsList, TabsTrigger } from '../shadcn/tabs'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { cn } from '../lib/utils'
import type { ValueBinding } from './value-binding'
import type { NoUndeclaredAria } from './aria-passthrough'

export interface SegmentedOption {
  label: React.ReactNode
  value: string
  disabled?: boolean
  /** Accessible name for an icon-only option (the label has no text of its own). */
  'aria-label'?: string
}

interface SegmentedBase {
  options: SegmentedOption[]
  onBlur?: () => void
  disabled?: boolean
  size?: 'sm' | 'default' | 'lg'
  name?: string
  id?: string
  invalid?: boolean
  className?: string
  'aria-label'?: string
  'aria-describedby'?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type SegmentedProps = SegmentedBase & NoUndeclaredAria<SegmentedBase> & ValueBinding<string>

// A segmented control is a Tabs header without panels: same nova-base TabsList /
// TabsTrigger styling (muted rail + raised active pill, themed for light & dark),
// single-select, always one value active.
export const Segmented = React.forwardRef<HTMLDivElement, SegmentedProps>(function Segmented(
  { options, value, defaultValue, onValueChange, onChange, onBlur, disabled, size, name, id, invalid, className,
    'aria-label': ariaLabel, 'aria-describedby': ariaDescribedby, 'data-testid': testid },
  ref,
) {
  const s = useSurface({ disabled, size })
  // Merged controlled/uncontrolled selection (keeps the hidden form input correct in both modes).
  const [current, setCurrent] = useControllableState<string>({
    value, defaultValue: defaultValue ?? '', onChange: (v) => { onValueChange?.(v); onChange?.(v) },
  })
  if (s.loading) return <Skeleton className={cn('h-8 w-full rounded-lg', className)} />
  return (
    <>
    {/* carry `name` for native form submission / name-based selectors (Tabs has none). */}
    {name != null && <input type="hidden" name={name} value={current} />}
    <Tabs
      ref={ref}
      value={current}
      onValueChange={(v) => setCurrent(String(v ?? ''))}
      data-testid={testid}
    >
      <TabsList
        id={id}
        onBlur={onBlur}
        aria-label={ariaLabel}
        aria-describedby={ariaDescribedby}
        aria-invalid={invalid || undefined}
        className={className}
      >
        {options.map((o) => (
          <TabsTrigger
            key={o.value}
            value={o.value}
            disabled={o.disabled || s.disabled || s.readOnly}
            aria-label={o['aria-label']}
            data-testid={`${testid}-opt-${o.value}`}
            // Stable, primitive-independent selected-state hook. Base UI marks the
            // active tab with a bare `data-active`; we additionally emit an explicit
            // `data-state="on"|"off"` (the old Radix vocabulary) so tests can assert
            // selection semantically without reaching into Base UI internals.
            data-state={current === o.value ? 'on' : 'off'}
            className={cn(s.size === 'sm' && 'px-2 py-1 text-xs')}
          >
            {o.label}
          </TabsTrigger>
        ))}
      </TabsList>
    </Tabs>
    </>
  )
})
