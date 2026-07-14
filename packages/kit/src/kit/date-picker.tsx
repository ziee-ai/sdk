import * as React from 'react'
import { format as formatDate, parseISO, isValid } from 'date-fns'
import { CalendarIcon } from 'lucide-react'
import { Popover as Root, PopoverTrigger, PopoverContent } from '../shadcn/popover'
import { Calendar } from '../shadcn/calendar'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// Coerce a Date | ISO-string | '' into a Date (or undefined). String values come from
// FormField bindings / JSON-schema forms; a Date comes from direct programmatic use.
function toDate(v: Date | string | undefined): Date | undefined {
  if (v == null || v === '') return undefined
  if (v instanceof Date) return isValid(v) ? v : undefined
  const d = parseISO(v)
  return isValid(d) ? d : undefined
}

// Single-date picker: a Button trigger + a Popover-hosted shadcn Calendar.
// Form-bindable: value + onChange (alias onValueChange) + name (hidden input) + id + ref(→trigger).
// `value` accepts a Date or an ISO string; the change handlers EMIT a string in `valueFormat`
// (default ISO `yyyy-MM-dd`) so RHF / JSON-schema forms get a stable serializable value.
export type DatePickerProps = {
  value?: Date | string
  defaultValue?: Date | string
  /** Emits the picked date as a string in `valueFormat` (or '' when cleared). */
  onChange?: (value: string) => void
  /** Alias of onChange (parallels onValueChange elsewhere in the kit). */
  onValueChange?: (value: string) => void
  onBlur?: () => void
  /** Trigger text when nothing is selected (required — caller owns it for i18n). */
  placeholder: string
  /** date-fns display format for the trigger label. Default 'PP' (e.g. Apr 29, 2024). */
  format?: string
  /** date-fns format the change handlers emit. Default 'yyyy-MM-dd' (ISO date). */
  valueFormat?: string
  /** Disable selecting any date before this one (forwarded to the calendar). */
  minDate?: Date
  /** Disable selecting any date after this one (forwarded to the calendar). */
  maxDate?: Date
  /** Predicate to disable arbitrary dates (legacy antd `disabledDate`). */
  disabledDate?: (date: Date) => boolean
  disabled?: boolean
  loading?: boolean
  invalid?: boolean
  size?: 'sm' | 'default' | 'lg'
  name?: string
  id?: string
  className?: string
  /** Accessible name for the trigger — REQUIRED (no default, for i18n). */
  'aria-label': string
  'aria-describedby'?: string
  'aria-labelledby'?: string
  'aria-required'?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
} & KitStyleProps

export const DatePicker = React.forwardRef<HTMLButtonElement, DatePickerProps>(function DatePicker(
  {
    value, defaultValue, onChange, onValueChange, onBlur, placeholder,
    format = 'PP', valueFormat = 'yyyy-MM-dd', minDate, maxDate, disabledDate,
    disabled, loading, invalid, size, name, id, className, style,
    'aria-label': ariaLabel, 'aria-describedby': ariaDescribedby,
    'aria-labelledby': ariaLabelledby, 'aria-required': ariaRequired,
    'data-testid': testid,
  },
  ref,
) {
  const s = useSurface({ disabled, size })
  const [open, setOpen] = React.useState(false)
  // Single source of truth: a string ('' = no selection) so the value stays serializable.
  const [current, setCurrent] = useControllableState<string>({
    value: value === undefined ? undefined : (toDate(value) ? formatDate(toDate(value)!, valueFormat) : ''),
    defaultValue: toDate(defaultValue) ? formatDate(toDate(defaultValue)!, valueFormat) : '',
    onChange: (v) => { onChange?.(v); onValueChange?.(v) },
  })
  // A disabled/loading trigger can't open at all; a readOnly surface stays openable but
  // ignores selection (parallels the other kit controls).
  const blocked = s.disabled || loading
  const selected = toDate(current)

  // react-day-picker disabled matchers: min/max bounds + an optional predicate.
  const calendarDisabled = React.useMemo(() => {
    const m: Array<{ before: Date } | { after: Date } | ((date: Date) => boolean)> = []
    if (minDate) m.push({ before: minDate })
    if (maxDate) m.push({ after: maxDate })
    if (disabledDate) m.push((date: Date) => disabledDate(date))
    return m
  }, [minDate, maxDate, disabledDate])

  if (s.loading) return <Skeleton className={cn('h-8', 'w-full rounded-lg', className)} />

  const choose = (d: Date | undefined) => {
    if (s.readOnly) return
    setCurrent(d ? formatDate(d, valueFormat) : '')
    setOpen(false)
  }

  return (
    <Root open={open} onOpenChange={(o) => { if (blocked) return; setOpen(o) }}>
      {/* native form submission (the trigger is a button with no value of its own). */}
      {name != null && <input type="hidden" name={name} value={current} />}
      <PopoverTrigger render={
        <button
          ref={ref}
          type="button"
          id={id}
          aria-label={ariaLabel}
          aria-describedby={ariaDescribedby}
          aria-labelledby={ariaLabelledby}
          aria-required={ariaRequired || undefined}
          aria-invalid={invalid || undefined}
          aria-readonly={s.readOnly || undefined}
          aria-haspopup="dialog"
          aria-expanded={open}
          data-testid={testid}
          disabled={blocked}
          // form onBlur fires only on real focus-leave of the trigger, never on popover close.
          onBlur={() => onBlur?.()}
          style={style}
          className={cn(
            'flex w-full items-center justify-between gap-2 rounded-lg border border-input bg-transparent px-2.5 py-1 text-sm',
            'focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 outline-none disabled:cursor-not-allowed disabled:opacity-50',
            'h-8', invalid && 'border-destructive focus-visible:ring-destructive', className,
          )}
        >
          <span className={cn('truncate', !selected && 'text-muted-foreground')}>
            {selected ? formatDate(selected, format) : placeholder}
          </span>
          <CalendarIcon className="size-4 shrink-0 opacity-50" aria-hidden />
        </button>
      } />
      <PopoverContent className="w-auto p-0" align="start">
        <Calendar
          mode="single"
          autoFocus
          selected={selected}
          onSelect={choose}
          disabled={calendarDisabled.length ? calendarDisabled : undefined}
        />
      </PopoverContent>
    </Root>
  )
})
