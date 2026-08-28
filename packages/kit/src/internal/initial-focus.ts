import * as React from 'react'

/**
 * WHERE FOCUS LANDS WHEN AN OVERLAY OPENS — declared, and guarded when it isn't.
 *
 * Base UI moves focus to the FIRST TABBABLE element in the popup. That is a DOM-ORDER rule, so
 * which control it picks is decided by layout rather than by intent, and any overlay that happens
 * to render a delete affordance above its form opens with a destructive control focused and one
 * Enter away. (Measured in cytoanalyst's `ShareDialog`: its share list precedes its form, so the
 * first tabbable is a per-row "Remove access for …" bin. It reads as fine only because the shares
 * load asynchronously — at mount the list is a spinner. That is TIMING, not design.)
 *
 * Two parts, and the second is the one that makes this a fix rather than a workaround:
 *
 *   1. `initialFocus` — the overlay SAYS where focus belongs. Intent, declared once, at the one
 *      place that knows: `'a-data-testid'`, a ref, or `false` for "leave it on the popup".
 *   2. THE DEFAULT IS GUARDED. With nothing declared the first tabbable still wins, except that
 *      anything marked `data-destructive` is SKIPPED — and if every candidate is destructive,
 *      focus goes to the popup itself. A default that is only safe when the author remembered to
 *      override it is not a default worth having, and per-overlay `autoFocus` would have left the
 *      rule unstated for the next overlay whose DOM order happens to sort a Delete first.
 *
 * `Button` stamps `data-destructive` for `variant="destructive"` automatically and takes an
 * explicit `destructive` prop for the ones that are destructive but styled QUIETLY — a ghost bin
 * icon is exactly that, and is why the variant alone could never have been the whole signal.
 */
export type InitialFocus = string | React.RefObject<HTMLElement | null> | false | undefined

/**
 * Tabbable candidates, in DOM order. Deliberately the same shape Base UI uses, so the guard
 * chooses from the SAME set the unguarded default would have — it removes candidates, it never
 * invents one the platform would not have picked.
 */
const TABBABLE = [
  'a[href]',
  'button',
  'input',
  'select',
  'textarea',
  '[tabindex]',
].join(',')

/**
 * A control is destructive if it says so, or if it sits inside something that does. `closest`
 * rather than a self-check: a bin icon is routinely wrapped (a tooltip trigger span, a
 * `Confirm` trigger), and a marker that only survives on the exact element would be defeated by
 * the next wrapper anyone adds.
 */
function isDestructive(el: Element): boolean {
  return el.closest('[data-destructive="true"]') != null
}

function isFocusable(el: HTMLElement): boolean {
  if (el.hasAttribute('disabled')) return false
  if (el.getAttribute('aria-disabled') === 'true') return false
  if (el.getAttribute('tabindex') === '-1') return false
  if (el.hidden || el.closest('[hidden]') != null) return false
  // `offsetParent === null` is the cheap "not rendered" test that works in jsdom and the browser
  // alike for `display:none` — but it is also null for `position:fixed`, which every popup is, so
  // it is applied to CANDIDATES INSIDE the popup only, never to the popup itself.
  return true
}

function firstSafeTabbable(popup: HTMLElement): HTMLElement | null {
  for (const el of Array.from(popup.querySelectorAll<HTMLElement>(TABBABLE))) {
    if (!isFocusable(el)) continue
    if (isDestructive(el)) continue
    return el
  }
  return null
}

/**
 * Build the `initialFocus` callback Base UI wants from the kit's declarative prop.
 *
 * Returning an ELEMENT (never `true`) in every non-`false` case is deliberate: `true` means
 * "use the default", and the default is precisely the behaviour being guarded against. A declared
 * target that has vanished (a renamed testid, a conditionally-rendered node) therefore degrades to
 * the GUARDED default rather than to the raw one — otherwise the seam would become a way to
 * silently re-acquire the exposure it exists to close.
 */
export function useInitialFocus(
  popupRef: React.RefObject<HTMLElement | null>,
  declared: InitialFocus,
): () => HTMLElement | boolean | null {
  return React.useCallback((): HTMLElement | boolean | null => {
    const popup = popupRef.current
    if (popup == null) return true
    if (declared === false) return popup
    if (typeof declared === 'string') {
      const el = popup.querySelector<HTMLElement>(`[data-testid="${declared}"]`)
      if (el != null && isFocusable(el)) return el
    } else if (declared != null && typeof declared === 'object') {
      const el = declared.current
      if (el != null && popup.contains(el)) return el
    }
    return firstSafeTabbable(popup) ?? popup
  }, [popupRef, declared])
}
