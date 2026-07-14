import * as React from 'react'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

type Gap = 'none' | 'xs' | 'sm' | 'md' | 'lg'
const gaps: Record<Gap, string> = { none: 'gap-0', xs: 'gap-1', sm: 'gap-2', md: 'gap-4', lg: 'gap-6' }
// accept legacy's gap value names too
const gapAlias: Record<string, Gap> = { small: 'sm', middle: 'md', large: 'lg' }
const aligns = { start: 'items-start', center: 'items-center', end: 'items-end', baseline: 'items-baseline', stretch: 'items-stretch' } as const
const justifies = { start: 'justify-start', center: 'justify-center', end: 'justify-end', between: 'justify-between', around: 'justify-around', evenly: 'justify-evenly' } as const

export type FlexProps = Omit<React.ComponentProps<'div'>, 'ref' | 'style'> & {
  /** Column or row. legacy `vertical` boolean is also accepted as an alias. */
  direction?: 'row' | 'column'
  /** legacy alias: `vertical` → direction="column". */
  vertical?: boolean
  align?: keyof typeof aligns
  justify?: keyof typeof justifies
  /** Kit names (xs/sm/md/lg) or legacy names (small/middle/large). */
  gap?: Gap | 'small' | 'middle' | 'large'
  wrap?: boolean
  inline?: boolean
} & KitStyleProps

export const Flex = React.forwardRef<HTMLDivElement, FlexProps>(function Flex(
  { direction, vertical, align, justify, gap = 'none', wrap, inline, style, allowStyle, className, ...props }, ref,
) {
  const gapKey = (gapAlias[gap] ?? gap) as Gap
  const col = vertical || direction === 'column'
  return (
    <div
      ref={ref}
      style={style}
      className={cn(
        inline ? 'inline-flex' : 'flex',
        col ? 'flex-col' : 'flex-row',
        align && aligns[align],
        justify && justifies[justify],
        gaps[gapKey],
        wrap && 'flex-wrap',
        className,
      )}
      {...props}
    />
  )
})
