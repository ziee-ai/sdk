import * as React from 'react'
import { Loader2 } from 'lucide-react'
import { Button as ButtonBase } from '../shadcn/button'
import { Skeleton } from '../shadcn/skeleton'
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'
import { useSurface } from './surface'
import { cn } from '../lib/utils'
import { safeHref } from './safe-href'

// v4 shadcn no longer exports a `ButtonProps` type (Button is a plain function
// component). Derive the base prop type from the component so it tracks the
// vendored primitive (variant/size/asChild + native button attrs).
type BaseButtonProps = React.ComponentProps<typeof ButtonBase>

type ButtonCommon = Omit<BaseButtonProps, 'size'> & {
  loading?: boolean
  /** Leading icon (legacy `icon`); rendered before children, replaced by the spinner while loading. */
  icon?: React.ReactNode
  /** Full-width block button (legacy `block`). */
  block?: boolean
  /** Render as an <a> styled as a button (pair with variant="link" for a text link). */
  href?: string
  target?: string
  /** Tooltip shown on hover AND keyboard focus. Doubles as the accessible name when a string. */
  tooltip?: React.ReactNode
  /** Side the built-in tooltip opens toward (default 'top'). Use 'bottom' for
   *  icon buttons in a panel/header top row so the tooltip drops into the body
   *  instead of clipping at the top edge or obscuring an adjacent control. */
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left'
  /** Test selector — REQUIRED, forwarded onto the rendered button/anchor via {...props} (i18n-safe). */
  'data-testid': string
}

// ─── Variant policy (Spec B — control-variant consistency) ────────────────────
// The design-critic pass repeatedly flagged buttons that pick their variant ad
// hoc. Pick a variant by ROLE, not by taste:
//
//   • Peer icon-only buttons in one chrome cluster (a viewer header, a card
//     toolbar, a drawer footer) MUST share ONE variant — default to `ghost`.
//     Don't mix `outline` + `ghost` side by side (e.g. the file-viewer header:
//     Copy/Download are `ghost` to match the drawer's `ghost` close button).
//   • A semantic action is colored to match the badge/outcome it produces:
//     Include → success (green), Exclude/remove → danger (red, `destructive`),
//     Unscreen / neutral reset → muted (`outline`/`ghost`). The action should
//     visually predict its result tag.
//   • The primary Save/submit is ALWAYS the saturated accent (`default`
//     variant) — never a weak `secondary`/`ghost` look. If it must be disabled
//     (e.g. a pristine form), keep the accent variant and add a tooltip that
//     explains WHY (see modules/settings SettingsFormActions.saveDisabledReason);
//     a greyed accent that explains itself reads as intentional, a greyed weak
//     variant reads as broken.
//   • A destructive singleton (a lone Delete) is `ghost` + danger tone, not a
//     filled red block that dominates the row.
//
// Icon-only buttons have no text → no accessible name. The type FORCES a `tooltip`
// when size="icon" (which also becomes the aria-label and shows on hover + focus).
export type ButtonProps =
  // icon-only has no text → force a name: a string tooltip, or an explicit aria-label.
  | (ButtonCommon & { size: 'icon'; tooltip: string })
  | (ButtonCommon & { size: 'icon'; 'aria-label': string; tooltip?: React.ReactNode })
  | (ButtonCommon & { size?: 'default' | 'lg' })

const skeletonH = (size?: BaseButtonProps['size']) =>
  size === 'lg' ? 'h-9' : 'h-8'

export const Button = React.forwardRef<HTMLButtonElement | HTMLAnchorElement, ButtonProps>(
  ({ loading, disabled, href, target, size: ownSize, type = 'button', tooltip, tooltipSide, icon, block, children, className: classNameProp, onClick, ...props }, ref) => {
    const { disabled: surfaceDisabled, loading: regionLoading, size: ambientSize } = useSurface({ disabled })
    const size = ownSize ?? ambientSize
    const className = cn(block && 'w-full', classNameProp)

    if (regionLoading) {
      return <Skeleton className={cn(skeletonH(size), 'w-20 rounded-md', className)} />
    }

    // surface-disabled → native `disabled` (truly inert). loading → keep focusable but
    // aria-disabled + block activation, so `aria-busy` is announced and focus isn't lost.
    const nativeDisabled = surfaceDisabled
    const isDisabled = surfaceDisabled || loading
    const ariaLabelProp = (props as { 'aria-label'?: string })['aria-label']
    // Suppressed when an outer kit <Tooltip> already wraps this button (it injects
    // data-tooltip-wrapped via Slot) — avoids a double tooltip popup. When that
    // marker carries a string, it is the outer tooltip's label.
    const tooltipWrapMarker = (props as Record<string, unknown>)['data-tooltip-wrapped']
    const tooltipWrapped = tooltipWrapMarker != null
    const iconOnly = icon != null && children == null
    // Adopt the wrapping <Tooltip content="X">'s label as the accessible name for
    // an otherwise-unnamed icon-only button — otherwise it has none (the visible
    // tooltip is not an accessible name). Only for icon-only; a text button keeps
    // its visible label as the name.
    const wrappedLabel =
      iconOnly && typeof tooltipWrapMarker === 'string' && tooltipWrapMarker.length > 0
        ? tooltipWrapMarker
        : undefined
    // a string tooltip becomes the accessible name (unless an explicit aria-label is given).
    const ariaLabel =
      ariaLabelProp ?? (typeof tooltip === 'string' ? tooltip : undefined) ?? wrappedLabel
    // Icon-only buttons (an icon, no visible text) should surface their accessible
    // name as a hover/focus tooltip too. If the caller gave an aria-label but no
    // explicit tooltip, reuse it — so every icon button has a tooltip without
    // per-call-site wiring.
    const effectiveTooltip =
      tooltip ?? (iconOnly && !tooltipWrapped && typeof ariaLabelProp === 'string' ? ariaLabelProp : undefined)
    const inner = (
      <>
        {loading ? <Loader2 className="animate-spin" aria-hidden /> : (icon != null && <span aria-hidden className="[&_svg]:size-4">{icon}</span>)}
        {children}
      </>
    )

    const linkHref = href ? safeHref(href) : undefined
    const node =
      linkHref && !isDisabled ? (
        <ButtonBase
          size={size}
          className={className}
          onClick={onClick as React.MouseEventHandler}
          // Rendering as an <a> (href): tell Base UI this is not a native
          // <button> so it doesn't warn/attach button-only semantics. Mirrors
          // shadcn/pagination.tsx's anchor case.
          nativeButton={false}
          {...props}
          render={
            <a
              ref={ref as React.Ref<HTMLAnchorElement>}
              href={linkHref}
              target={target}
              rel={target === '_blank' ? 'noopener noreferrer' : undefined}
              aria-label={ariaLabel}
            >
              {inner}
            </a>
          }
        />
      ) : (
        <ButtonBase
          ref={ref as React.Ref<HTMLButtonElement>}
          type={type}
          size={size}
          disabled={nativeDisabled}
          aria-disabled={loading || undefined}
          aria-busy={loading || undefined}
          aria-label={ariaLabel}
          className={cn(className, loading && 'pointer-events-none opacity-70')}
          // while loading: stay focusable but swallow activation.
          onClick={loading ? (e) => e.preventDefault() : (onClick as React.MouseEventHandler)}
          {...props}
        >
          {inner}
        </ButtonBase>
      )

    if (effectiveTooltip == null) return node
    return (
      <TooltipProvider delay={300}>
        <Tooltip>
          <TooltipTrigger render={node} />
          <TooltipContent side={tooltipSide}>{effectiveTooltip}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
    )
  },
)
Button.displayName = 'Button'
