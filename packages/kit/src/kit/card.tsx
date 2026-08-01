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

/**
 * Props for {@link CardActions}. Plain div props minus `style` (inline style is
 * gated kit-wide).
 *
 * NOTE — this row is OPINIONATED about its direct children, which is unusual for
 * a kit container, so it is stated here. (It is NOT in `KIT_MANIFEST.md`: that
 * generator emits per-PROP rows only, and this type declares no kit-authored
 * props, so the manifest entry is just "_No always-required props._" — read this
 * comment, not the manifest, before using it.) Every direct `button`/`a` child is
 * normalized to `max-w-full`,
 * `h-auto min-h-8 py-1`, and wrapping text (`whitespace-normal wrap-anywhere
 * text-center` — `wrap-anywhere`, not `break-words`, because only the former
 * participates in min-content sizing and so actually breaks an unbroken token). That is what stops a single over-wide action from protruding out
 * of the row. Consequences to know before using it:
 *
 * - a `size="lg"` (36px) or `size="icon*"` (square) child is re-sized to the 32px
 *   `size="default"` metric and is no longer square — put icon-only or `lg`
 *   actions in a plain row, not here;
 * - the rules reach DIRECT children only, so a nested wrapper's buttons are NOT
 *   protected — make every action a direct child (use `me-auto` on a leading
 *   action to split the row) rather than grouping them in a nested `div`;
 * - non-control children are deliberately untouched, so a `Text`/status node in
 *   the row keeps its own metrics;
 * - `className` merges last, so `justify-*`/`items-*` can be overridden; the
 *   child rules can be overridden only by re-declaring the same arbitrary variant
 *   (e.g. `[&>button]:min-h-10`), NOT by a height utility on the row.
 */
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
 * Mechanism worth knowing: the child rules compile to `& > :is(button,a)`, whose
 * specificity is the parent class (0,1,0) plus `:is(button,a)` (0,0,1) = (0,1,1)
 * — so they beat the button's own `.h-8` (0,1,0) by SPECIFICITY. (An earlier,
 * unscoped `[&>*]` form did NOT: `*` adds nothing, so that version tied at
 * (0,1,0) and won only by Tailwind's emission order.) A row that needs different
 * child metrics must therefore re-declare the same variant
 * (`[&>button]:min-h-10`), not set a height on the row. See {@link CardActionsProps}
 * for the full child contract.
 */
export function CardActions({ className, children, ...rest }: CardActionsProps) {
  return (
    <div
      className={cn(
        'flex w-full flex-wrap justify-end gap-2',
        // An action wider than the line is capped to it and wraps its label,
        // rather than overflowing the inline-start edge where nothing can reach it.
        // Scoped to CONTROLS (`button`/`a`): applying these to every child leaked
        // padding and a min-height onto plain layout/text nodes, which grew rows
        // at EVERY width and broke this component's "inert when it fits" contract.
        // `wrap-anywhere` (overflow-wrap: anywhere), NOT `break-words`: measured on
        // a real button in this row, `overflow-wrap: break-word` leaves an
        // unbroken 47-char token at clientWidth 236 / scrollWidth 312 with the
        // height unchanged at 32px — the word never breaks, because break-word is
        // excluded from min-content sizing, so the label spills straight back out
        // of the card's `overflow-hidden` edge. `anywhere` IS included in
        // min-content sizing: same token measures 236/236 and wraps to 50px tall.
        // That distinction is the whole difference between this row keeping its
        // promise and quietly reproducing the defect for a non-English label.
        // `text-center` keeps a wrapped 2-line label centred rather than ragged,
        // since the button centres as a flex box but does not centre its text.
        '[&>:is(button,a)]:max-w-full [&>:is(button,a)]:h-auto [&>:is(button,a)]:min-h-8',
        '[&>:is(button,a)]:py-1 [&>:is(button,a)]:whitespace-normal [&>:is(button,a)]:wrap-anywhere [&>:is(button,a)]:text-center',
        className,
      )}
      {...rest}
      // AFTER the spread on purpose: the slot marker is the only selector this
      // row is addressed by, so a caller must not be able to clobber it.
      data-slot="card-actions"
    >
      {children}
    </div>
  )
}
