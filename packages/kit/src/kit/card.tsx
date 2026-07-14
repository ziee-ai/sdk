import * as React from 'react'
import { Card as Base, CardHeader, CardTitle, CardContent, CardFooter } from '../shadcn/card'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

// Omit native `title` (we take a ReactNode title) + `style` (style-gated). The rest of the
// div props (onClick, data-*, role, id, aria-*) pass through to the card root.
export type CardProps = Omit<React.ComponentProps<'div'>, 'title' | 'style'> & {
  title?: React.ReactNode
  /** Top-right actions. */
  extra?: React.ReactNode
  footer?: React.ReactNode
  /** Container is loading → skeleton body (also triggered by an ambient loading surface). */
  loading?: boolean
  size?: 'sm' | 'default'
  /** Lift + shadow on hover (legacy `hoverable`). */
  hoverable?: boolean
  className?: string
  children?: React.ReactNode
  /** Test selector — REQUIRED, forwarded onto the card root via {...rest} (i18n-safe). */
  'data-testid': string
} & KitStyleProps

export function Card({ title, extra, footer, loading, size = 'default', hoverable, className, style, allowStyle: _a, children, ...rest }: CardProps) {
  const s = useSurface({})
  const skeleton = loading || s.loading
  const pad = size === 'sm' ? 'px-4' : undefined
  return (
    <Base
      style={style}
      className={cn(size === 'sm' && 'gap-3 py-4', hoverable && 'transition-shadow hover:shadow-md', rest.onClick && 'cursor-pointer', className)}
      {...rest}
    >
      {/* Single row at every width: title (claims the free width, ellipsizes
          when long) + right-aligned extra. Stacking title-over-extra on mobile
          read as premature wrapping for the common short-title + short-extra
          case; a genuinely long title now truncates rather than forcing a
          mid-word wrap or a stack. */}
      {(title != null || extra != null) && (
        <CardHeader className={cn('flex flex-row items-center justify-between gap-2', pad)}>
          {title != null ? (
            <CardTitle className="min-w-0 flex-1 truncate">{title}</CardTitle>
          ) : (
            <span />
          )}
          {extra}
        </CardHeader>
      )}
      <CardContent className={pad}>
        {skeleton ? (
          <div className="space-y-2" aria-busy>
            <Skeleton className="h-4 w-3/4" />
            <Skeleton className="h-4 w-1/2" />
            <Skeleton className="h-4 w-2/3" />
          </div>
        ) : (
          children
        )}
      </CardContent>
      {footer != null && <CardFooter className={pad}>{footer}</CardFooter>}
    </Base>
  )
}
