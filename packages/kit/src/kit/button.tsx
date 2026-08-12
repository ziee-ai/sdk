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
  /**
   * WHY this control is unavailable. Presence means unavailable — the string IS the reason,
   * rather than a boolean plus a separate message that can disagree with it. (The
   * `removeBlockedReason` idiom, cytoanalyst commit 93167985.)
   *
   * USE THIS INSTEAD OF `disabled` WHENEVER THERE IS A REASON TO GIVE. A natively `disabled`
   * button is `pointer-events:none` AND out of the tab order, so it can open no tooltip and
   * take no focus: a reason parked on one is unreachable through every channel a user has.
   * This prop renders the control `aria-disabled` with its click handler dropped, so it stays
   * hoverable and focusable, and then wires the reason into BOTH channels:
   *
   *   · the TOOLTIP, for a pointer user  (it replaces the tooltip for as long as it is set);
   *   · an sr-only node referenced by `aria-describedby`, for everyone else.
   *
   * The control's NAME is untouched — `aria-label`, or the `tooltip` string when there is no
   * `aria-label`. That separation is the point: the reason says why it refused, the name still
   * says WHICH control refused, and a screen-reader user needs both.
   *
   * `disabled` remains correct for a control with NO reason to give (a pristine form's Save
   * beside its own validation summary), and for a genuinely inert region-wide state.
   */
  unavailableReason?: string
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
function visibleText(node: React.ReactNode): string {
  if (node == null || typeof node === 'boolean') return ''
  if (typeof node === 'string') return node
  if (typeof node === 'number' || typeof node === 'bigint') return String(node)
  if (Array.isArray(node)) return node.map(visibleText).join('')
  if (React.isValidElement(node)) {
    const p = node.props as { children?: React.ReactNode; 'aria-hidden'?: boolean | string }
    if (p['aria-hidden'] === true || p['aria-hidden'] === 'true') return ''
    return visibleText(p.children)
  }
  if (typeof node === 'object' && Symbol.iterator in (node as object)) {
    return Array.from(node as Iterable<React.ReactNode>).map(visibleText).join('')
  }
  return ''
}

function hasVisibleText(node: React.ReactNode): boolean {
  return visibleText(node).trim() !== ''
}

/**
 * A natively `disabled` button can open no React tooltip, so the kit refuses to pretend otherwise.
 *
 * TWO independent reasons, either one sufficient: the kit's own `buttonVariants` sets
 * `disabled:pointer-events-none`, and even without it a disabled control's mouse events do not
 * propagate to the document root — which is where React listens, because it delegates there. A
 * document-level listener records ZERO `mouseover` on a disabled button, capture and bubble alike.
 *
 * It does not drop the tooltip. Dropping it would delete the caller's declared intent and hand the
 * consumer's shrink-only tooltip ledger a fresh crop of "offenders" for controls that were already
 * mute — no user is better off. It names the control and the remedy, and is deduped so a re-render
 * storm cannot bury it.
 *
 * IT IS A `console.error`, NOT A `console.warn`, AND THAT IS THE POINT. A warn is advisory: it
 * scrolled past in a dev console and 13 live sites shipped a sentence no user could read. The
 * consumer's only console-reading gate (`scripts/preland-frontend-health.mjs`, which gates on
 * `console-error` findings) therefore could not see this class at all. Escalating it is what turns
 * "a message someone might notice" into "the branch does not land" — done only AFTER every live
 * site was fixed, because escalating first just paints the gate red for work already known.
 */
const inertTooltipsWarned = new Set<string>()
function warnInertTooltip(testid: string | undefined, label: unknown): void {
  const key = `${testid ?? '?'}::${String(label)}`
  if (inertTooltipsWarned.has(key)) return
  inertTooltipsWarned.add(key)
  console.error(
    `[@ziee/kit] <Button data-testid="${testid ?? '(none)'}"> is natively \`disabled\` AND carries a ` +
      `tooltip (${JSON.stringify(String(label))}). That tooltip can never open: a disabled button is ` +
      `pointer-events:none and its mouse events never reach the document root, where React listens. ` +
      `If the control must explain WHY it is unavailable, pass \`unavailableReason="…"\` instead of ` +
      `\`disabled\` — it renders the control aria-disabled with its click handler dropped, so it stays ` +
      `hoverable and focusable, and wires the reason into the tooltip AND aria-describedby. If there ` +
      `is genuinely no reason to give, drop the \`tooltip\` and keep \`disabled\`.`,
  )
}

export const Button = React.forwardRef<HTMLButtonElement | HTMLAnchorElement, ButtonProps>(
  ({ loading, disabled, unavailableReason, href, target, size: ownSize, type = 'button', tooltip, tooltipSide, icon, block, children, className: classNameProp, onClick, id, ...props }, ref) => {
    const { disabled: surfaceDisabled, loading: regionLoading, size: ambientSize } = useSurface({ disabled })
    const size = ownSize ?? ambientSize
    // An UNAVAILABLE control is aria-disabled, never natively disabled — that is the whole
    // point of the prop (see its doc). `useId` rather than a hand-built key: the reason node's
    // `id` must be UNIQUE IN THE DOCUMENT or the browser silently resolves `aria-describedby`
    // to whichever node came first, and one row's reason announces another row's. The
    // hand-rolled precedent needed a paragraph of commentary to get that right once; every
    // call site after it would have had to get it right again.
    const unavailable = unavailableReason != null && unavailableReason !== ''
    const whyId = React.useId()
    const className = cn(block && 'w-full', unavailable && 'cursor-not-allowed opacity-50', classNameProp)

    if (regionLoading) {
      return <Skeleton className={cn(skeletonH(size), 'w-20 rounded-md', className)} />
    }

    // surface-disabled → native `disabled` (truly inert). loading → keep focusable but
    // aria-disabled + block activation, so `aria-busy` is announced and focus isn't lost.
    // `unavailableReason` joins `loading` on the aria-disabled side, and OVERRIDES a native
    // disable: a caller that supplies a reason has said the control must be able to state it.
    const nativeDisabled = surfaceDisabled && !unavailable
    const isDisabled = surfaceDisabled || loading || unavailable
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
    //
    // THE UNAVAILABLE FALLBACK IS NOT OPTIONAL, AND IT WAS MISSING. The `sr-only` reason node
    // below lives INSIDE the button (a sibling wrapper would change the layout of every flex rail
    // this renders in), and the accessible name of a button with no `aria-label` is ITS TEXT
    // CONTENT — which includes that node. So a TEXT button carrying a reason and no explicit
    // label was named "NextFill in Alpha channel, Beta channel": the reason swallowed the name,
    // which is the exact failure `unavailableReason` was written to prevent.
    //
    // It went unseen because every shipped caller happened to be an icon button (named by its
    // string `tooltip`) or a text button that had been given an explicit `aria-label` — and
    // because this prop's own doc asserted "aria-label is ALWAYS set when a reason is", which the
    // kit's suite had only ever checked on the icon-only path. Found by a consumer test on the
    // first text-button caller. The visible words are the name whenever nothing better was given.
    const ariaLabel =
      ariaLabelProp ??
      (typeof tooltip === 'string' ? tooltip : undefined) ??
      wrappedLabel ??
      (unavailable ? visibleText(children).replace(/\s+/g, ' ').trim() || undefined : undefined)
    // Icon-only buttons (an icon, no visible text) should surface their accessible
    // name as a hover/focus tooltip too. If the caller gave an aria-label but no
    // explicit tooltip, reuse it — so every icon button has a tooltip without
    // per-call-site wiring.
    // While UNAVAILABLE the hover text is the REASON, not the name — the name is what the
    // control is called and is already carried by `aria-label`; the reason is the thing a user
    // staring at a greyed control actually needs and has no other way to get.
    const effectiveTooltip = unavailable
      ? unavailableReason
      : (tooltip ?? (iconOnly && !tooltipWrapped && typeof ariaLabelProp === 'string' ? ariaLabelProp : undefined))
    const inner = (
      <>
        {loading ? <Loader2 className="animate-spin" aria-hidden /> : (icon != null && <span aria-hidden className="[&_svg]:size-4">{icon}</span>)}
        {children}
        {/* The reason in the ACCESSIBILITY tree. INSIDE the button rather than in a sibling
            wrapper: a wrapper would change the layout of every flex rail this renders in, and
            an `sr-only` node is out of flow and contributes no visible text (the icon-only
            predicate above and the consumer's DOM sweep both skip it). It never becomes the
            NAME either — `aria-label` is always set when a reason is (asserted in the kit's
            own suite), and text content is only a fallback after that. */}
        {unavailable && (
          <span id={whyId} className="sr-only">
            {unavailableReason}
          </span>
        )}
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
          aria-disabled={loading || unavailable || undefined}
          aria-busy={loading || undefined}
          aria-label={ariaLabel}
          className={cn(className, loading && 'pointer-events-none opacity-70')}
          // while loading OR unavailable: stay focusable but swallow activation. `aria-disabled`
          // is a claim to assistive tech, not an enforcement — without this the control would
          // still fire, which is the trap that makes people reach for native `disabled`.
          onClick={
            loading || unavailable ? (e) => e.preventDefault() : (onClick as React.MouseEventHandler)
          }
          id={id}
          {...props}
          // AFTER the spread: the reason node this button points at is the kit's, and a caller's
          // own `aria-describedby` must not silently replace it. Both are kept.
          aria-describedby={
            unavailable
              ? [(props as { 'aria-describedby'?: string })['aria-describedby'], whyId]
                  .filter(Boolean)
                  .join(' ')
              : (props as { 'aria-describedby'?: string })['aria-describedby']
          }
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
