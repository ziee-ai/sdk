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

/**
 * Does this subtree render WORDS a sighted user can read?
 *
 * This is what decides `iconOnly`, and it decides it the way the CONSUMER'S GUARD decides it —
 * `visibleText(el) === ''` off the rendered DOM (`tests/gallery-e2e/icon-button-tooltips.spec.ts`).
 * Keying icon-only-ness off anything else is how the defect happened: `iconOnly` used to be
 * `icon != null && children == null`, so an icon passed as a CHILD disqualified the button from
 * the aria-label→tooltip promotion, and `<Button size="icon" aria-label="Close"><X /></Button>`
 * rendered a mute glyph while satisfying both the prop union and KIT_MANIFEST.md.
 *
 * The two allowances match the guard's, for the same reasons:
 *   · an `<svg>` is the icon, and it carries no readable child text anyway;
 *   · `aria-hidden` content is decoration BY DECLARATION — counting it as the label would let
 *     any glyph opt out of ever explaining itself.
 *
 * KNOWN LIMIT, stated rather than hidden: a component child that renders text from its own props
 * (`<Trans i18nKey="save" />`) has no children to walk, so it reads as no-text. The blast radius
 * of being wrong is bounded — a false "icon-only" only does anything when the caller ALSO supplied
 * an `aria-label` or wrapped the button in a `<Tooltip>`, i.e. only when a name exists to show. It
 * was measured at zero call sites across both consuming repos at the time of writing.
 */
function hasVisibleText(node: React.ReactNode): boolean {
  if (node == null || typeof node === 'boolean') return false
  if (typeof node === 'string') return node.trim() !== ''
  if (typeof node === 'number' || typeof node === 'bigint') return true
  if (Array.isArray(node)) return node.some(hasVisibleText)
  if (React.isValidElement(node)) {
    const p = node.props as { children?: React.ReactNode; 'aria-hidden'?: boolean | string }
    if (p['aria-hidden'] === true || p['aria-hidden'] === 'true') return false
    return hasVisibleText(p.children)
  }
  if (typeof node === 'object' && Symbol.iterator in (node as object)) {
    return Array.from(node as Iterable<React.ReactNode>).some(hasVisibleText)
  }
  return false
}

/**
 * A natively `disabled` button can open no React tooltip, so the kit refuses to pretend otherwise.
 *
 * TWO independent reasons, either one sufficient: the kit's own `buttonVariants` sets
 * `disabled:pointer-events-none`, and even without it a disabled control's mouse events do not
 * propagate to the document root — which is where React listens, because it delegates there. A
 * document-level listener records ZERO `mouseover` on a disabled button, capture and bubble alike.
 *
 * It WARNS rather than dropping the tooltip. Dropping it would delete the caller's declared intent
 * and hand the consumer's shrink-only tooltip ledger a fresh crop of "offenders" for controls that
 * were already mute — no user is better off. The warning names the control and the remedy, and is
 * deduped so a re-render storm cannot bury it.
 */
const inertTooltipsWarned = new Set<string>()
function warnInertTooltip(testid: string | undefined, label: unknown): void {
  const key = `${testid ?? '?'}::${String(label)}`
  if (inertTooltipsWarned.has(key)) return
  inertTooltipsWarned.add(key)
  console.warn(
    `[@ziee/kit] <Button data-testid="${testid ?? '(none)'}"> is natively \`disabled\` AND carries a ` +
      `tooltip (${JSON.stringify(String(label))}). That tooltip can never open: a disabled button is ` +
      `pointer-events:none and its mouse events never reach the document root, where React listens. ` +
      `If the control must explain WHY it is unavailable, make it \`aria-disabled\` with no click ` +
      `handler (it stays hoverable and focusable), or hang the tooltip on a focusable wrapper span.`,
  )
}

export const Button = React.forwardRef<HTMLButtonElement | HTMLAnchorElement, ButtonProps>(
  ({ loading, disabled, href, target, size: ownSize, type = 'button', tooltip, tooltipSide, icon, block, children, className: classNameProp, onClick, id, ...props }, ref) => {
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
    // ICON-ONLY = renders no readable words. NOT "children == null" — an icon passed as a child
    // is still an icon, and that mismatch is what made a whole class of buttons mute. See
    // `hasVisibleText` above for why this mirrors the consumer guard's DOM predicate.
    const iconOnly = !hasVisibleText(children) && (icon != null || children != null || size === 'icon')
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
          id={id}
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
          id={id}
          {...props}
        >
          {inner}
        </ButtonBase>
      )

    if (effectiveTooltip == null) return node
    if (process.env.NODE_ENV !== 'production' && nativeDisabled) {
      warnInertTooltip((props as { 'data-testid'?: string })['data-testid'], effectiveTooltip)
    }
    return (
      <TooltipProvider delay={300}>
        <Tooltip>
          {/* `id` is HOISTED onto the trigger, not merely left on the rendered node.
              Tooltip.Trigger keys its store on `useBaseUiId(idProp)` — the id it was GIVEN — and
              base-ui then merges the rendered element's own props OVER its computed ones. So an
              id arriving on this Button (a Popover/Dropdown/Confirm trigger injects one; a caller
              may pass one) lands on the DOM node while the tooltip stays registered under an id
              nothing on screen carries, and hovering opens nothing — with
              `data-base-ui-tooltip-trigger` still stamped, so both source and markers read fine.
              Bisected one prop at a time in icon-button-tooltip.test.tsx: `id` alone does this. */}
          <TooltipTrigger id={id} render={node} />
          <TooltipContent side={tooltipSide}>{effectiveTooltip}</TooltipContent>
        </Tooltip>
      </TooltipProvider>
    )
  },
)
Button.displayName = 'Button'
