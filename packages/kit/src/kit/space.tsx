import * as React from 'react'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

type Size = 'xs' | 'sm' | 'md' | 'lg'
const gaps: Record<Size, string> = { xs: 'gap-1', sm: 'gap-2', md: 'gap-4', lg: 'gap-6' }
const sizeAlias: Record<string, Size> = { small: 'sm', middle: 'md', large: 'lg' }

// legacy Space: inline gap container. direction='vertical' stacks; wraps by default in horizontal.
// `size` accepts kit names (xs/sm/md/lg), legacy names (small/middle/large), a pixel number, or
// a [columnGap, rowGap] tuple (numbers → inline gap on the component's own element).
export type SpaceProps = Omit<React.ComponentProps<'div'>, 'ref' | 'style'> & {
  direction?: 'horizontal' | 'vertical'
  size?: Size | 'small' | 'middle' | 'large' | number | [number, number]
  align?: 'start' | 'center' | 'end' | 'baseline'
  wrap?: boolean
} & KitStyleProps

const aligns = { start: 'items-start', center: 'items-center', end: 'items-end', baseline: 'items-baseline' } as const

export const Space = React.forwardRef<HTMLDivElement, SpaceProps>(function Space(
  { direction = 'horizontal', size = 'sm', align, wrap = true, style, allowStyle, className, ...props }, ref,
) {
  const vertical = direction === 'vertical'
  // numeric / tuple sizes → inline gap (component's own style, not a consumer style prop).
  const numeric = typeof size === 'number' || Array.isArray(size)
  const gapStyle: React.CSSProperties | undefined = Array.isArray(size)
    ? { columnGap: size[0], rowGap: size[1] }
    : typeof size === 'number'
      ? { gap: size }
      : undefined
  const gapClass = numeric ? undefined : gaps[(sizeAlias[size as string] ?? size) as Size]
  return (
    <div
      ref={ref}
      style={{ ...gapStyle, ...style }}
      className={cn(
        'inline-flex',
        vertical ? 'flex-col' : 'flex-row',
        gapClass,
        align ? aligns[align] : !vertical && 'items-center',
        !vertical && wrap && 'flex-wrap',
        className,
      )}
      {...props}
    />
  )
})
