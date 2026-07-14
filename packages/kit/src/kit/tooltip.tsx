import * as React from 'react'
import { Slot } from '@radix-ui/react-slot'
import { Tooltip as TT, TooltipTrigger, TooltipContent, TooltipProvider } from '../shadcn/tooltip'

/** Extends `HTMLAttributes` (NOT a `[key: string]: unknown` index signature):
 *  any DOM prop/event a parent `asChild` slot injects (onClick/ref/… from a
 *  DropdownMenuTrigger, PopoverTrigger, …) is accepted and forwarded onto the
 *  child via Slot, so <Tooltip> composes directly inside a trigger. A string
 *  index signature here would collapse `keyof` to `string` and make
 *  `forwardRef`'s mapped props type widen EVERY named prop to `unknown`. */
export interface TooltipProps
  extends Omit<React.HTMLAttributes<HTMLElement>, 'title' | 'content'> {
  /** Tooltip body. `title` is an accepted alias (legacy uses `title`). */
  content?: React.ReactNode
  title?: React.ReactNode
  side?: 'top' | 'right' | 'bottom' | 'left'
  delay?: number
  className?: string
  children: React.ReactElement
}

export const Tooltip = React.forwardRef<HTMLElement, TooltipProps>(function Tooltip(
  { content, title, side = 'top', delay = 300, className, children, ...rest },
  ref,
) {
  const body = content ?? title
  // When the body is a plain string, carry it in the marker so a wrapped
  // icon-only kit <Button> can adopt it as its accessible name (aria-label)
  // — an outer <Tooltip content="X"> is otherwise the ONLY label an icon
  // button has, yet it never reached the DOM as a name (a11y-name failures).
  // Empty string still marks "wrapped" (presence, not value, suppresses the
  // Button's own auto-tooltip); a non-empty value additionally names it.
  const marker = typeof body === 'string' ? body : ''
  // Merge any parent-injected props (e.g. an asChild trigger's onClick/ref)
  // onto the child so composition works regardless of nesting order.
  const child = (
    // Mark the child so a wrapped kit <Button> suppresses its own aria-label
    // auto-tooltip — this <Tooltip> already owns the tooltip (no double popup).
    <Slot data-tooltip-wrapped={marker} ref={ref as React.Ref<HTMLElement>} {...rest}>
      {children}
    </Slot>
  )
  if (body == null) return child
  return (
    <TooltipProvider delay={delay}>
      <TT>
        <TooltipTrigger render={child} />
        <TooltipContent side={side} className={className}>{body}</TooltipContent>
      </TT>
    </TooltipProvider>
  )
})
