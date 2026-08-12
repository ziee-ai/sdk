import * as React from 'react'
import { cn } from '../lib/utils'

/**
 * "WE KEPT SOMETHING OTHER THAN WHAT YOU GAVE US" — said out loud, once, where it happened.
 *
 * ── The class ──────────────────────────────────────────────────────────────────────────────
 * A refusal that explains itself is the house standard. A SILENT REWRITE is worse: the user is
 * not merely refused without a reason, they are not told they were refused at all. The control
 * accepts the interaction, changes the value, and shows a result that looks like what they asked
 * for. `InputNumber` clamped a typed 999 to 100 on blur; `Upload` took the first of three dropped
 * files and dropped two on the floor. Both looked like success.
 *
 * ── The rule this component implements ─────────────────────────────────────────────────────
 *   1. NORMALIZATION — same value, different spelling (`"1."`→`1`, `" x "`→`"x"`, a case-folded
 *      search query). SILENT. The user cannot perceive a difference, so a message is pure noise
 *      and trains them to ignore the messages that matter.
 *   2. CLAMP / ROUND / REVERT — the kept value is one the user can perceive as different from
 *      what they entered. ANNOUNCE. Note it is NOT an error: the interaction succeeded, so an
 *      invalid state would be a lie. It is a notice.
 *   3. DISCARD — data dropped rather than adjusted (files past the first, filtered-out tokens,
 *      characters past a cap). ANNOUNCE WHAT WAS KEPT AND WHAT WAS DROPPED, with counts. "kept 1,
 *      ignored 2" is actionable; "one file appeared" is not.
 *
 * The dividing question is always "could the user tell?". If yes, tell them first.
 *
 * ── Why the region is ALWAYS mounted ───────────────────────────────────────────────────────
 * An `aria-live` region that is inserted into the DOM together with its text is not reliably
 * announced — screen readers watch an EXISTING region for changes. So the node is always
 * rendered, and `sr-only` while empty. `sr-only` is `position:absolute`, which is not a flex or
 * grid item, so an empty notice adds no box, no row and no gap: mounting it costs the layout of
 * every control that adopts it exactly nothing, which is what makes "always mounted" affordable.
 */
export function ClampNotice({ message, className }: { message: string | null; className?: string }) {
  return (
    <span
      data-slot="clamp-notice"
      role="status"
      aria-live="polite"
      className={cn(
        message == null || message === ''
          ? 'sr-only'
          : cn('text-xs text-muted-foreground', className),
      )}
    >
      {message ?? ''}
    </span>
  )
}
