import * as React from 'react'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// legacy Statistic: a titled numeric display with optional prefix/suffix + precision.
export type StatisticProps = {
  title: React.ReactNode
  value: number | string
  precision?: number
  prefix?: React.ReactNode
  suffix?: React.ReactNode
  /** Intl locale grouping for numeric values (e.g. thousands separators). */
  groupSeparator?: boolean
  className?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
  valueClassName?: string} & KitStyleProps

export function Statistic({ title, value, precision, prefix, suffix, groupSeparator = true, className, valueClassName, style, 'data-testid': testid }: StatisticProps) {
  let display: React.ReactNode = value
  if (typeof value === 'number') {
    display = precision != null
      ? value.toLocaleString(undefined, { minimumFractionDigits: precision, maximumFractionDigits: precision, useGrouping: groupSeparator })
      : value.toLocaleString(undefined, { useGrouping: groupSeparator })
  }
  return (
    <div className={cn('flex flex-col gap-1', className)} style={style} data-testid={testid}>
      <span className="text-sm text-muted-foreground">{title}</span>
      <span className={cn('flex items-baseline gap-1 text-2xl font-semibold tabular-nums', valueClassName)}>
        {prefix != null && <span className="text-base">{prefix}</span>}
        {display}
        {suffix != null && <span className="text-base text-muted-foreground">{suffix}</span>}
      </span>
    </div>
  )
}
