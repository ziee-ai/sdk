import * as React from 'react'
import { Loader2 } from 'lucide-react'
import { Textarea as Base } from '../shadcn/textarea'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

export type TextareaProps = Omit<React.ComponentProps<'textarea'>, 'style'> & {
  loading?: boolean
  invalid?: boolean
  /** Grow with content (legacy `autoSize`). Optional min/max row bounds. */
  autoSize?: boolean | { minRows?: number; maxRows?: number }
  /** Test selector — REQUIRED, forwarded onto the textarea via {...props} (i18n-safe). */
  'data-testid': string
} & KitStyleProps

export const Textarea = React.forwardRef<HTMLTextAreaElement, TextareaProps>(
  ({ loading, invalid, disabled, readOnly, autoSize, style, allowStyle: _a, className, rows = 4, ...props }, ref) => {
    const s = useSurface({ disabled, readOnly })
    if (s.loading) {
      return <Skeleton className={cn('w-full rounded-md', className)} style={{ height: `${rows * 1.5 + 1}rem` }} />
    }
    // autoSize → CSS content sizing; min/max rows become height bounds (1.5rem ≈ 1 line).
    const auto = autoSize != null && autoSize !== false
    const bounds = typeof autoSize === 'object' ? autoSize : undefined
    const autoStyle: React.CSSProperties | undefined = auto
      ? {
          ...(bounds?.minRows ? { minHeight: `${bounds.minRows * 1.5 + 0.75}rem` } : {}),
          ...(bounds?.maxRows ? { maxHeight: `${bounds.maxRows * 1.5 + 0.75}rem` } : {}),
        }
      : undefined
    const field = (
      <Base
        ref={ref}
        rows={auto ? undefined : rows}
        disabled={s.disabled || loading}
        readOnly={s.readOnly}
        aria-invalid={invalid || undefined}
        aria-busy={loading || undefined}
        style={{ ...autoStyle, ...style }}
        className={cn(auto && '[field-sizing:content]', className, invalid && 'border-destructive focus-visible:ring-destructive')}
        {...props}
        // A controlled textarea must never receive null/undefined (React warns +
        // Base UI flips uncontrolled↔controlled). Form bindings pass a `value` that
        // may be null before data loads → coerce to ''. Uncontrolled use (no `value`
        // prop, only defaultValue) is left untouched.
        {...('value' in props ? { value: props.value ?? '' } : {})}
      />
    )
    // own `loading` → in-place spinner (region loading uses the skeleton above).
    if (!loading) return field
    return (
      <div className="relative w-full">
        {field}
        <Loader2 className="absolute right-3 top-3 size-4 animate-spin opacity-70" aria-hidden />
      </div>
    )
  },
)
Textarea.displayName = 'Textarea'
