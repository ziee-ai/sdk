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

// Omit `style` (style-gated, like CardProps). A plain action row needs no test id
// of its own — it is addressed by `[data-slot="card-actions"]`.
export type CardActionsProps = Omit<React.ComponentProps<'div'>, 'style'>

/**
 * `CardActions` — the action row for a Card `footer`, which CANNOT clip its own
 * controls.
 *
 * The card-shaped sibling of `DialogFooter` / `AlertDialogFooter`, which already
 * refuse to let a footer action row overflow. A card footer had no such
 * primitive, so every call site hand-rolled `flex w-full justify-end gap-2` — and
 * that row is a trap, not merely untidy:
 *
 *   A flex row that overflows while `justify-content: flex-end` pushes the
 *   overflow out of its **inline-START** edge. There it is (a) clipped to zero
 *   width by the Card root's `overflow-hidden`, and (b) UNREACHABLE — a start-edge
 *   overflow creates no scrollable region, so `scrollWidth === clientWidth` and no
 *   gesture can reveal it. Measured on the MCP tool-approval card at a 390px
 *   viewport: of Deny / Approve once / Approve for this conversation, the first
 *   two had `visibleW=0` and did not hit-test, leaving the BROADEST approval as
 *   the only pressable control. On a consent surface that is a safety defect, not
 *   a cosmetic one.
 *
 * The rule here is content-driven, NOT breakpoint-driven, and that is deliberate:
 * a card's action row lives in containers whose width is independent of the
 * viewport (a virtualized message list that indents the row, split panes, side
 * panels). A `sm:` breakpoint would report "wide" while the container is narrow
 * and reintroduce the bug. `flex-wrap` is self-tuning — completely inert whenever
 * the row fits, so a desktop-width card renders exactly as it did before.
 *
 * `flex-wrap` alone is not sufficient. A kit `Button` carries `shrink-0
 * whitespace-nowrap`, so a SINGLE action wider than the line cannot shrink and
 * would still protrude (taxonomy B2 "failure-to-wrap — content clipped/protruding
 * where wrap/ellipsis was possible"). The child rules below cap such an action to
 * the line and let its LABEL wrap instead — never truncated and never ellipsised,
 * because on a consent surface hidden text is the thing being defended against.
 *
 * Contract worth knowing: the child rules out-specify a child's own size utility
 * (`.row > *` beats `.min-h-9`), so children are normalized to a 32px minimum
 * height — i.e. `size="default"`. A row that needs taller controls sets the
 * height on the row via `className`.
 */
export function CardActions({ className, children, ...rest }: CardActionsProps) {
  return (
    <div
      data-slot="card-actions"
      className={cn(
        'flex w-full flex-wrap justify-end gap-2',
        // An action wider than the line is capped to it and wraps its label,
        // rather than overflowing the inline-start edge where nothing can reach it.
        '[&>*]:max-w-full [&>*]:h-auto [&>*]:min-h-8 [&>*]:py-1 [&>*]:whitespace-normal',
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  )
}
