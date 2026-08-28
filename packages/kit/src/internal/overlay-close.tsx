import { Dialog as DialogPrimitive } from '@base-ui/react/dialog'
import { XIcon } from 'lucide-react'
import { Button } from '../shadcn/button'
import { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'

/**
 * The × in the corner of a Dialog or a Sheet.
 *
 * ONE component for both because they were byte-identical and both were mute: an `<XIcon/>` plus
 * `<span class="sr-only">Close</span>`, which is a perfectly good accessible NAME and is rendered
 * by nothing. A sighted user who does not read the glyph as "close" got no hover text at all — the
 * consuming app's rendered-DOM sweep counted this exact control on NINE gallery surfaces
 * (`tests/gallery-e2e/icon-button-tooltips-baseline.json`, ledger row `Close: 9`).
 *
 * The tooltip hangs on a Tooltip.Trigger that RENDERS the Dialog.Close, rather than on the Close
 * itself: two Base UI triggers on one element compose correctly in that direction (the outer one
 * owns the `id` its store is keyed on), which is the same contract `kit/button.tsx` restores by
 * hoisting `id` onto its own trigger. Verified in `kit/icon-button-tooltip.test.tsx`.
 *
 * `label` feeds BOTH channels from one string, so a screen reader and a pointer can never be told
 * different things about the same ×.
 */
export function OverlayCloseButton({
  slot,
  label,
  className,
}: {
  /** `dialog-close` / `sheet-close` — the structural marker callers select on. */
  slot: string
  /** Accessible name AND hover text. */
  label: string
  className: string
}) {
  return (
    <TooltipProvider delay={300}>
      <Tooltip>
        <TooltipTrigger
          render={
            <DialogPrimitive.Close
              data-slot={slot}
              render={<Button variant="ghost" className={className} size="icon-sm" />}
            >
              <XIcon />
              <span className="sr-only">{label}</span>
            </DialogPrimitive.Close>
          }
        />
        <TooltipContent>{label}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
