import * as React from 'react'
import { X } from 'lucide-react'
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

// legacy Tag → kit Tag. Semantic tones (NOT arbitrary hex). Optional close button is
// a real <button> with a forced accessible name.
export type TagTone = 'default' | 'primary' | 'success' | 'warning' | 'error' | 'info'
/** Fill style. soft (default) = tinted bg; solid = saturated bg (legacy `filled`);
 *  outline = transparent bg + colored border (legacy `outlined`/`borderless`). */
export type TagVariant = 'soft' | 'solid' | 'outline'

// Tones use the dark-aware SEMANTIC status tokens (--success/--warning/--info +
// --destructive for error), NOT raw Tailwind palette hues — so text contrast
// meets WCAG AA in BOTH themes (the tokens are AA-tuned as text on the page bg
// in index.css). The /10 fill stays faint enough that the token text reads on it.
const tones: Record<TagTone, string> = {
  default: 'bg-muted text-foreground/80 border-transparent',
  primary: 'bg-primary/10 text-primary border-primary/20',
  success: 'bg-success/10 text-success border-success/25',
  warning: 'bg-warning/10 text-warning border-warning/25',
  error: 'bg-destructive/10 text-destructive border-destructive/25',
  info: 'bg-info/10 text-info border-info/25',
}
const solidTones: Record<TagTone, string> = {
  default: 'bg-foreground/80 text-background border-transparent',
  primary: 'bg-primary text-primary-foreground border-transparent',
  success: 'bg-success text-success-foreground border-transparent',
  warning: 'bg-warning text-warning-foreground border-transparent',
  error: 'bg-destructive text-destructive-foreground border-transparent',
  info: 'bg-info text-info-foreground border-transparent',
}
const outlineTones: Record<TagTone, string> = {
  default: 'bg-transparent text-foreground/80 border-border',
  primary: 'bg-transparent text-primary border-primary/40',
  success: 'bg-transparent text-success border-success/45',
  warning: 'bg-transparent text-warning border-warning/45',
  error: 'bg-transparent text-destructive border-destructive/45',
  info: 'bg-transparent text-info border-info/45',
}
const variantTones: Record<TagVariant, Record<TagTone, string>> = {
  soft: tones, solid: solidTones, outline: outlineTones,
}

type TagBase = Omit<React.ComponentProps<'span'>, 'ref' | 'style'> & {
  tone?: TagTone
  variant?: TagVariant
  icon?: React.ReactNode
  /** Test selector — REQUIRED, forwarded onto the tag span via {...props} (i18n-safe). */
  'data-testid': string
} & KitStyleProps
// A closable Tag MUST supply both onClose and an explicit closeLabel (no default — the
// caller owns the string for i18n).
export type TagProps =
  | (TagBase & { onClose?: undefined; closeLabel?: never })
  | (TagBase & { onClose: () => void; closeLabel: string })

// React.memo: Tags are rendered in large lists (MultiSelect tags, table cells) where the parent
// re-renders for unrelated reasons — memo skips unchanged tags.
const TagInner = React.forwardRef<HTMLSpanElement, TagProps>(function Tag(
  { tone = 'default', variant = 'soft', icon, className, children, ...rest }, ref,
) {
  const onClose = (rest as { onClose?: () => void }).onClose
  const closeLabel = (rest as { closeLabel?: string }).closeLabel
  const props = (() => {
    // strip kit-only props so they don't leak onto the DOM span.
    const { onClose: _o, closeLabel: _c, allowStyle: _a, ...p } = rest as Record<string, unknown>
    return p
  })()
  return (
    <span
      ref={ref}
      data-slot="tag"
      className={cn(
        'inline-flex items-center gap-1 rounded-md border px-2 py-0.5 text-xs font-medium',
        // Keep tag content on a single line; when the row is narrow the whole
        // tag wraps to the next line via the parent's flex-wrap, rather than
        // breaking the tag's own text.
        'whitespace-nowrap',
        variantTones[variant][tone],
        className,
      )}
      {...props}
    >
      {icon != null && <span className="[&_svg]:size-3" aria-hidden>{icon}</span>}
      {children}
      {/* Icon-only, so `closeLabel` is shown on hover as well as exposed as the name. A × on a
          tag is the most ambiguous glyph in the kit — "remove this tag" vs "close this thing" —
          and in a MultiSelect there is one per value, so the label is also what tells them apart.
          `data-slot="tag-close"` is preserved through the tooltip trigger (base-ui merges the
          RENDERED element's props last), which `MultiSelect.stopFromTagClose` depends on. */}
      {onClose != null && (
        <TooltipProvider delay={300}>
          <Tooltip>
            <TooltipTrigger
              render={
                <button
                  type="button"
                  // The tag's own structural marker, beside the `data-slot="tag"` on its span. A
                  // container that embeds a closable Tag has to be able to tell "the × was pressed"
                  // from "the tag was pressed" — those mean different things and only the first is
                  // the tag's business (see `MultiSelect`'s `stopFromTagClose`, where conflating
                  // them made a bound combobox unopenable).
                  data-slot="tag-close"
                  onClick={onClose}
                  aria-label={closeLabel}
                  data-testid={props['data-testid'] != null ? `${props['data-testid'] as string}-close` : undefined}
                  className="relative -me-0.5 ms-0.5 inline-flex items-center justify-center rounded-sm opacity-60 hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
                >
                  <X className="size-3" aria-hidden />
                </button>
              }
            />
            <TooltipContent>{closeLabel}</TooltipContent>
          </Tooltip>
        </TooltipProvider>
      )}
    </span>
  )
})
export const Tag = React.memo(TagInner)
