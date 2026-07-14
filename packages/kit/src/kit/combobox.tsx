import * as React from 'react'
import {
  Combobox as Root,
  ComboboxInput,
  ComboboxContent,
  ComboboxList,
  ComboboxItem,
  ComboboxEmpty,
} from '../shadcn/combobox'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { type KitStyleProps } from './style-guard'
import type { ValueBinding } from './value-binding'
import { cn } from '../lib/utils'

export interface ComboboxOption {
  label: string
  value: string
  disabled?: boolean
}

// Searchable single-select. Composes the shadcn `combobox` base (Base UI Combobox):
// the field shows the selected label and filters in place as you type. The kit adds
// the app surface (loading/disabled/size), string value binding (value/onChange), a
// REQUIRED data-testid, and i18n labels. Base UI owns the filtering + list windowing.
interface ComboboxBase {
  options: ComboboxOption[]
  onBlur?: () => void
  /** Field text when nothing is selected (required — caller owns it for i18n). */
  placeholder: string
  /** Kept for API compatibility; the base combobox filters in the same field, so the
   *  `placeholder` above is what's shown. */
  searchPlaceholder?: string
  /** Shown when the filter matches nothing (required — i18n). */
  emptyText: string
  disabled?: boolean
  loading?: boolean
  invalid?: boolean
  /** Kept for API compatibility — Base UI Combobox windows large lists itself. */
  virtual?: boolean
  size?: 'sm' | 'default' | 'lg'
  name?: string
  id?: string
  className?: string
  'aria-describedby'?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  /** Test selector — forwarded onto the field (i18n-safe). Options derive `${testid}-opt-${value}`. */
  'data-testid': string
}
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type ComboboxProps = ComboboxBase & ValueBinding<string> & KitStyleProps

export const Combobox = React.forwardRef<HTMLInputElement, ComboboxProps>(function Combobox(
  {
    options, value, defaultValue, onValueChange, onChange, onBlur, placeholder, emptyText,
    disabled, loading, invalid, size, name, id, className, style, allowStyle: _a,
    'aria-describedby': ariaDescribedby, 'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby,
    'data-testid': testid,
    // accepted for API compatibility, not needed by the base combobox:
    searchPlaceholder: _sp, virtual: _v,
  },
  ref,
) {
  const s = useSurface({ disabled, size })
  const locked = s.disabled || loading || s.readOnly
  // Single source of truth as a string; map to/from the option object the base wants.
  const [current, setCurrent] = useControllableState<string>({
    value, defaultValue: defaultValue ?? '', onChange: (v) => { onValueChange?.(v); onChange?.(v) },
  })
  const byValue = React.useMemo(() => new Map(options.map((o) => [o.value, o])), [options])
  const selected = current ? byValue.get(current) ?? null : null

  if (s.loading) return <Skeleton className={cn('h-8 w-full rounded-lg', className)} />

  return (
    <Root
      items={options}
      value={selected}
      onValueChange={(o: ComboboxOption | null) => setCurrent(o ? o.value : '')}
      itemToStringLabel={(o: ComboboxOption) => o.label}
      itemToStringValue={(o: ComboboxOption) => o.label}
      name={name}
      disabled={locked}
    >
      <ComboboxInput
        ref={ref}
        id={id}
        onBlur={onBlur}
        placeholder={placeholder}
        showClear
        aria-invalid={invalid || undefined}
        aria-describedby={ariaDescribedby}
        aria-label={ariaLabel}
        aria-labelledby={ariaLabelledby}
        data-testid={testid}
        style={style}
        className={cn('w-full', className)}
      />
      <ComboboxContent>
        <ComboboxEmpty>{emptyText}</ComboboxEmpty>
        <ComboboxList>
          {(o: ComboboxOption) => (
            <ComboboxItem
              key={o.value}
              value={o}
              disabled={o.disabled}
              data-testid={testid ? `${testid}-opt-${o.value}` : undefined}
            >
              {o.label}
            </ComboboxItem>
          )}
        </ComboboxList>
      </ComboboxContent>
    </Root>
  )
})
Combobox.displayName = 'Combobox'
