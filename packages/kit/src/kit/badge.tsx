import * as React from 'react'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

export type BadgeTone = 'neutral' | 'primary' | 'success' | 'warning' | 'error' | 'info'

const tones: Record<BadgeTone, string> = {
  neutral: 'bg-muted text-muted-foreground',
  primary: 'bg-primary/10 text-primary',
  success: 'bg-green-500/10 text-green-700 dark:text-green-400',
  warning: 'bg-amber-500/10 text-amber-700 dark:text-amber-500',
  error: 'bg-red-500/10 text-red-700 dark:text-red-400',
  info: 'bg-blue-500/10 text-blue-700 dark:text-blue-400',
}

// Solid fills for the notification-count bubble (a count needs high contrast, not the
// soft tag tint above). Count mode defaults to `error` (the conventional red badge).
const countTones: Record<BadgeTone, string> = {
  neutral: 'bg-muted-foreground text-background',
  primary: 'bg-primary text-primary-foreground',
  success: 'bg-green-700 text-white',
  warning: 'bg-amber-500 text-amber-950',
  error: 'bg-red-700 text-white',
  info: 'bg-blue-600 text-white',
}

type BadgeBase = Omit<React.ComponentProps<'span'>, 'prefix' | 'style'> & {
  tone?: BadgeTone
  /** Test selector — REQUIRED, forwarded onto the badge root via {...props} (i18n-safe). */
  'data-testid': string
} & KitStyleProps

// Tag mode (default): a soft-tinted label badge. No count/dot props.
type BadgeTagProps = BadgeBase & {
  icon?: React.ReactNode
  count?: never
  dot?: never
  overflowCount?: never
  showZero?: never
  offset?: never
}

// Count/dot overlay mode: wraps `children` (or renders standalone). An accessible name is
// REQUIRED — a bare number/dot is a context-free SR announcement (kit no-silent-default rule).
type BadgeCountBase = BadgeBase & {
  icon?: never
  /** Cap the displayed number; counts above show `${overflowCount}+`. Default 99. */
  overflowCount?: number
  /** Render the bubble even when `count` is 0 (legacy `showZero`). Default false. */
  showZero?: boolean
  /** Shift the corner bubble from the top-right by [x, y] px (legacy `offset`). */
  offset?: [number, number]
  /** Accessible name for the indicator — REQUIRED in count/dot mode. */
  'aria-label': string
}

export type BadgeProps =
  | BadgeTagProps
  // count present (numeric bubble); dot may also force the dot rendering.
  | (BadgeCountBase & { count: number; dot?: boolean })
  // dot present without a count.
  | (BadgeCountBase & { dot: true; count?: number })

export const Badge = React.forwardRef<HTMLSpanElement, BadgeProps>(
  (
    { tone = 'neutral', icon, count, dot, overflowCount = 99, showZero = false, offset,
      style, allowStyle, className, children, ...props },
    ref,
  ) => {
    // Overlay (count/dot) mode — wraps `children` with a positioned bubble.
    if (count != null || dot) {
      const bubbleTone = countTones[tone === 'neutral' ? 'error' : tone]
      const numeric = count ?? 0
      const display = dot ? null : numeric > overflowCount ? `${overflowCount}+` : String(numeric)
      // A zero (or negative) count hides the bubble unless showZero (dots always show).
      const hideBubble = !dot && numeric <= 0 && !showZero
      const hasLabel = props['aria-label'] != null

      const dotCls = cn('inline-block size-2 rounded-full', bubbleTone)
      const pillCls = cn(
        'inline-flex min-w-4 items-center justify-center rounded-full px-1 text-[10px] font-medium leading-4 tabular-nums',
        bubbleTone,
      )

      // Standalone (no wrapped content) → render the bubble inline.
      if (children == null) {
        if (hideBubble) return null
        return dot ? (
          <span
            ref={ref}
            role={hasLabel ? 'status' : undefined}
            aria-hidden={hasLabel ? undefined : true}
            style={style}
            className={cn(dotCls, className)}
            {...props}
          />
        ) : (
          <span ref={ref} role="status" style={style} className={cn(pillCls, className)} {...props}>
            {display}
          </span>
        )
      }

      // Corner overlay: center the bubble on the top-right corner, then apply `offset`.
      const cornerStyle: React.CSSProperties = {
        transform: `translate(50%, -50%)${offset ? ` translate(${offset[0]}px, ${offset[1]}px)` : ''}`,
      }
      const bubble = hideBubble ? null : (
        <span
          role={dot ? (hasLabel ? 'status' : undefined) : 'status'}
          aria-label={props['aria-label']}
          aria-hidden={dot && !hasLabel ? true : undefined}
          style={cornerStyle}
          className={cn('absolute right-0 top-0 ring-2 ring-background', dot ? dotCls : pillCls)}
        >
          {display}
        </span>
      )

      // aria-label is consumed by the bubble (it describes the count), so strip it off the
      // wrapper to avoid a duplicate announcement.
      const { 'aria-label': _label, ...wrapperProps } = props
      return (
        <span ref={ref} style={style} className={cn('relative inline-flex', className)} {...wrapperProps}>
          {children}
          {bubble}
        </span>
      )
    }

    // Tag mode (unchanged): a soft-tinted label badge.
    return (
      <span
        ref={ref}
        data-slot="badge"
        style={style}
        className={cn(
          'inline-flex items-center gap-1 whitespace-nowrap rounded-md px-2 py-0.5 text-xs font-medium [&_svg]:size-3',
          tones[tone],
          className,
        )}
        {...props}
      >
        {icon != null && <span aria-hidden>{icon}</span>}
        {children}
      </span>
    )
  },
)
Badge.displayName = 'Badge'
