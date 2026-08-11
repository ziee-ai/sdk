import * as React from 'react'
import { Button } from './button'
import { Tooltip } from './tooltip'

/**
 * Does this trigger child actually render a real `<button>`?
 *
 * Base UI's triggers default to `nativeButton: true` and log an ERROR when the rendered element
 * disagrees with the flag ("A component that acts as a button expected a non-<button> because the
 * `nativeButton` prop is false…"). `Popover` and `Dropdown` therefore each guessed from the
 * child's TYPE, and each guessed the same way — key off identity, because a component child
 * cannot be introspected for the tag it will render.
 *
 * THE CASE THAT BROKE. A kit `<Tooltip>` is TRANSPARENT: it renders its child through a Slot, so
 * the tag that lands in the DOM is the tooltip's CHILD. Wrapping a trigger's `<button>` in one —
 * which became the normal way to tooltip a trigger once the id hoist made that composition work —
 * turned `childType` from the string `'button'` into the `Tooltip` component, the heuristic said
 * "not a native button", and every render of the workbench tab strip logged a Base UI error.
 * Measured by CytoAnalyst's runtime-health pass: 6 gating HIGH findings on each of 5 workbench
 * surfaces, on a branch whose typecheck, lints, unit suites, gallery sweep and e2e were all green.
 *
 * So the answer is resolved THROUGH the transparent wrapper rather than at its surface.
 */
export function rendersNativeButton(child: React.ReactNode): boolean {
  if (!React.isValidElement(child)) return false
  const type = child.type
  if (type === Tooltip) {
    return rendersNativeButton((child.props as { children?: React.ReactNode }).children)
  }
  if (typeof type === 'string') return type === 'button'
  // The kit Button is the one component known to render a native <button>.
  return type === Button
}
