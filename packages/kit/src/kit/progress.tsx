import * as React from 'react'
import { Progress as Base } from '../shadcn/progress'
import { cn } from '../lib/utils'

export type ProgressTone = 'primary' | 'success' | 'warning' | 'error'
// Colour ONLY the indicator (the filled portion). The DOM is
// Root > Track > Indicator, and Base UI sizes the indicator via an inline
// `width: <percent>%`. Targeting `[&>div]` (the Track) instead painted the
// whole track with the fill colour, so every bar looked 100% full regardless
// of value — key off the indicator's `data-slot` so the muted track shows
// through for the empty portion.
const toneCls: Record<ProgressTone, string> = {
  primary: '[&_[data-slot=progress-indicator]]:bg-primary',
  success: '[&_[data-slot=progress-indicator]]:bg-green-600',
  warning: '[&_[data-slot=progress-indicator]]:bg-amber-500',
  error: '[&_[data-slot=progress-indicator]]:bg-destructive',
}

const strokeCls: Record<ProgressTone, string> = {
  primary: 'text-primary', success: 'text-green-600', warning: 'text-amber-500', error: 'text-destructive',
}

export interface ProgressProps {
  value: number
  tone?: ProgressTone
  size?: 'sm' | 'default'
  /** Render as a circular gauge instead of a bar (legacy `type="circle"`). */
  shape?: 'line' | 'circle'
  /** Diameter in px for the circular shape (legacy circle `size`). Default 120. */
  circleSize?: number
  /** Show the percentage text beside the bar (legacy `showInfo`). */
  showInfo?: boolean
  /** Custom label formatter (legacy `format`), e.g. (p) => `${p}%`. Implies showInfo. */
  format?: (percent: number) => React.ReactNode
  className?: string
  /** Required accessible name (no default — caller owns the string for i18n). */
  'aria-label': string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}

export function Progress({ value, tone = 'primary', size = 'default', shape = 'line', circleSize = 120, showInfo, format, className, 'aria-label': ariaLabel, 'data-testid': testid }: ProgressProps) {
  const v = Math.max(0, Math.min(100, Number.isFinite(value) ? value : 0))
  if (shape === 'circle') {
    const stroke = size === 'sm' ? 6 : 8
    const r = (circleSize - stroke) / 2
    const c = 2 * Math.PI * r
    // showInfo defaults to ON for the circle (the number lives in the centre); pass showInfo={false} to hide.
    const showCenter = showInfo !== false
    return (
      <div
        className={cn('relative inline-flex items-center justify-center', className)}
        style={{ width: circleSize, height: circleSize }}
        role="progressbar" aria-label={ariaLabel} aria-valuenow={Math.round(v)} aria-valuemin={0} aria-valuemax={100}
        data-testid={testid}
      >
        <svg width={circleSize} height={circleSize} className={cn('-rotate-90', strokeCls[tone])}>
          <circle cx={circleSize / 2} cy={circleSize / 2} r={r} fill="none" strokeWidth={stroke} className="stroke-muted" />
          <circle
            cx={circleSize / 2} cy={circleSize / 2} r={r} fill="none" strokeWidth={stroke}
            stroke="currentColor" strokeLinecap="round"
            strokeDasharray={c} strokeDashoffset={c * (1 - v / 100)}
            style={{ transition: 'stroke-dashoffset 300ms' }}
          />
        </svg>
        {showCenter && (
          <span className="absolute text-sm font-medium tabular-nums">
            {format ? format(v) : `${Math.round(v)}%`}
          </span>
        )}
      </div>
    )
  }
  const bar = (
    <Base
      value={v}
      aria-label={ariaLabel}
      aria-valuetext={`${Math.round(v)}%`}
      data-testid={!showInfo && !format ? testid : undefined}
      className={cn(toneCls[tone], size === 'sm' && 'h-1.5', !showInfo && !format && className)}
    />
  )
  if (!showInfo && !format) return bar
  return (
    <div className={cn('flex items-center gap-2', className)} data-testid={testid}>
      <div className="flex-1">{bar}</div>
      <span className="shrink-0 text-sm tabular-nums text-muted-foreground">
        {format ? format(v) : `${Math.round(v)}%`}
      </span>
    </div>
  )
}
