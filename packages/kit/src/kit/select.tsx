import * as React from 'react'
import { Loader2, X } from 'lucide-react'
import {
  Select as SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  SelectGroup,
  SelectLabel,
} from '../shadcn/select'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { cn } from '../lib/utils'
import type { ValueBinding } from './value-binding'

export interface SelectOption {
  label: React.ReactNode
  value: string
  disabled?: boolean
  /** What to show in the TRIGGER when this option is selected, if it should differ from
   *  `label` (the dropdown-row content). Falls back to `label`. */
  selectedLabel?: React.ReactNode
}
export interface SelectOptionGroup {
  label?: React.ReactNode
  options: SelectOption[]
}

// Surface: region loading → skeleton · own loading → trigger spinner · disabled · readOnly · size.
// Form-bindable: value + onChange(value) (alias of onValueChange) + onBlur + name + id + ref(→trigger)
// + aria-* passthrough so FormField's aria-describedby/required reach the trigger.
// Custom render: `optionRender` controls each dropdown row; per-option `selectedLabel`
// (or `labelRender`) controls the selected display in the trigger — so the row and the
// selected value can render differently (legacy optionRender / labelRender / optionLabelProp).
interface SelectBase {
  options: (SelectOption | SelectOptionGroup)[]
  onBlur?: () => void
  placeholder?: string
  disabled?: boolean
  loading?: boolean
  invalid?: boolean
  size?: 'sm' | 'default' | 'lg'
  name?: string
  id?: string
  className?: string
  /** Custom content for each dropdown row. Receives the option. */
  optionRender?: (option: SelectOption) => React.ReactNode
  /** Custom content for the selected value in the trigger. Receives the selected option
   *  (undefined if none). Overrides per-option `selectedLabel`. */
  labelRender?: (option: SelectOption | undefined) => React.ReactNode
  /** Constrain the dropdown to the trigger's width (legacy `popupMatchSelectWidth`).
   *  Default true (exact match); false lets the dropdown grow wider for long options. */
  popupMatchSelectWidth?: boolean
  'aria-describedby'?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-required'?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). Options derive `${testid}-opt-${value}`. */
  'data-testid': string
}
// allowClear adds a clear button → its accessible name (clearLabel) is REQUIRED (no default, for i18n).
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type SelectProps = SelectBase &
  ValueBinding<string> &
  ({ allowClear?: false; clearLabel?: never } | { allowClear: true; clearLabel: string })

const isGroup = (o: SelectOption | SelectOptionGroup): o is SelectOptionGroup =>
  Array.isArray((o as SelectOptionGroup).options)

function flatOptions(options: (SelectOption | SelectOptionGroup)[]): SelectOption[] {
  return options.flatMap((o) => (isGroup(o) ? o.options : [o]))
}

export const Select = React.forwardRef<HTMLButtonElement, SelectProps>(function Select(
  {
    options, value, defaultValue, onValueChange, onChange, onBlur, placeholder,
    disabled, loading, invalid, size, name, id, className, optionRender, labelRender, popupMatchSelectWidth = true,
    'aria-describedby': ariaDescribedby, 'aria-label': ariaLabel,
    'aria-labelledby': ariaLabelledby, 'aria-required': ariaRequired,
    'data-testid': testid,
    ...rest
  },
  ref,
) {
  const allowClear = (rest as { allowClear?: boolean }).allowClear
  const clearLabel = (rest as { clearLabel?: string }).clearLabel
  const s = useSurface({ disabled, size })
  // Single source of truth (drives Radix via value={current} below), so clear + custom display +
  // selection never desync. '' = no selection (kept a string so Radix stays controlled).
  const [current, setCurrent] = useControllableState<string>({
    value, defaultValue: defaultValue ?? '', onChange: (v) => { onValueChange?.(v); onChange?.(v) },
  })
  const handleChange = (v: string) => setCurrent(v)
  const clear = () => setCurrent('')
  // Radix Select has no readOnly — map an ambient readOnly surface to "can't change".
  const locked = s.disabled || loading || s.readOnly

  const items = React.useMemo(
    () =>
      options.map((o, i) =>
        isGroup(o) ? (
          <SelectGroup key={o.options[0]?.value ?? `g${i}`} className="p-0">
            {/* p-0 cancels base-nova's group inset (the popup now owns p-1). */}
            {o.label != null && <SelectLabel>{o.label}</SelectLabel>}
            {o.options.map((opt) => (
              <SelectItem
                key={opt.value}
                value={opt.value}
                disabled={opt.disabled}
                data-testid={testid ? `${testid}-opt-${opt.value}` : undefined}
              >
                {optionRender ? optionRender(opt) : opt.label}
              </SelectItem>
            ))}
          </SelectGroup>
        ) : (
          <SelectItem
            key={o.value}
            value={o.value}
            disabled={o.disabled}
            data-testid={testid ? `${testid}-opt-${o.value}` : undefined}
          >
            {optionRender ? optionRender(o) : o.label}
          </SelectItem>
        ),
      ),
    [options, optionRender, testid],
  )

  // O(1) selected-option lookup (replaces a per-render flatOptions().find()).
  const optionByValue = React.useMemo(() => {
    const m = new Map<string, SelectOption>()
    for (const o of flatOptions(options)) m.set(o.value, o)
    return m
  }, [options])

  if (s.loading) {
    return <Skeleton className={cn('h-9 w-full rounded-md', className)} />
  }

  // Custom selected display: a per-option selectedLabel or a labelRender means the trigger
  // must show something other than the row text → render controlled SelectValue children.
  const selectedOpt = current ? optionByValue.get(current) : undefined
  // base-ui's SelectValue renders the raw VALUE when given no children, so a plain
  // option would show its value ("light") instead of its label ("Light"). Always
  // drive the trigger from the option's label (or an explicit override).
  const customDisplay =
    labelRender != null
      ? labelRender(selectedOpt)
      : selectedOpt?.selectedLabel != null
        ? selectedOpt.selectedLabel
        : selectedOpt?.label
  const showClear = allowClear && current !== '' && !locked

  return (
    <SelectRoot value={current} onValueChange={(v) => handleChange(v ?? '')} disabled={locked} name={name}>
      {/* relative wrapper so the clear button is a SIBLING of the trigger, never a <button>
          nested inside the trigger <button> (invalid HTML + keyboard-unreachable). */}
      <div className="relative">
        <SelectTrigger
          ref={ref}
          id={id}
          onBlur={onBlur}
          aria-invalid={invalid || undefined}
          aria-busy={loading || undefined}
          aria-describedby={ariaDescribedby}
          aria-label={ariaLabel}
          aria-labelledby={ariaLabelledby}
          aria-required={ariaRequired}
          data-testid={testid}
          size={s.size === 'sm' ? 'sm' : 'default'}
          className={cn('w-full', className, showClear && 'pr-14')}
        >
          <SelectValue placeholder={placeholder}>{customDisplay}</SelectValue>
          {loading && <Loader2 className="ml-2 size-4 shrink-0 animate-spin opacity-70" aria-hidden />}
        </SelectTrigger>
        {showClear && (
          <button
            type="button"
            aria-label={clearLabel}
            data-testid={testid ? `${testid}-clear` : undefined}
            onClick={clear}
            // pointer-down stop so clearing via mouse doesn't also open the Radix dropdown.
            onPointerDown={(e) => { e.preventDefault(); e.stopPropagation() }}
            className="absolute right-7 top-1/2 inline-flex -translate-y-1/2 items-center justify-center rounded-sm opacity-60 hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          >
            <X className="size-3.5" aria-hidden />
          </button>
        )}
      </div>
      {/* p-1: base-nova puts the list inset on SelectGroup only, so flat
          (ungrouped) items would sit flush against the popup edge — restore it
          on the popup. match=false → let the popup grow past the trigger width
          (Base UI defaults the popup to the trigger's `--anchor-width`). */}
      <SelectContent
        data-testid={testid ? `${testid}-popup` : undefined}
        className={cn('p-1', !popupMatchSelectWidth && 'w-auto min-w-(--anchor-width)')}
      >
        {items}
      </SelectContent>
    </SelectRoot>
  )
})
