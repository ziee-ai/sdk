import * as React from 'react'
import { Popover as Root, PopoverTrigger, PopoverContent } from '../shadcn/popover'
import { Button } from './button'

export interface PopoverProps {
  content: React.ReactNode
  /** Optional header inside the popover (legacy `title`). */
  title?: React.ReactNode
  children: React.ReactElement
  open?: boolean
  onOpenChange?: (open: boolean) => void
  side?: 'top' | 'right' | 'bottom' | 'left'
  align?: 'start' | 'center' | 'end'
  /** 'click' (default) or 'hover'. legacy defaults to hover; Radix is click-native, so hover
   *  is emulated via pointer/focus on the trigger (uncontrolled only). */
  trigger?: 'click' | 'hover'
  className?: string
}

export function Popover({ content, title, children, open, onOpenChange, side, align, trigger = 'click', className }: PopoverProps) {
  const [internal, setInternal] = React.useState(false)
  const controlled = open !== undefined
  const isOpen = controlled ? open : internal
  const setOpen = (o: boolean) => { if (!controlled) setInternal(o); onOpenChange?.(o) }
  const hover = trigger === 'hover' && !controlled
  const hoverHandlers = hover
    ? { onMouseEnter: () => setInternal(true), onMouseLeave: () => setInternal(false) }
    : {}
  // Base UI's trigger defaults to nativeButton=true and warns if the rendered
  // element isn't a real <button>. The hover wrapper is a <span>; otherwise the
  // caller's child may be a native <button>, the kit <Button> (renders one), or a
  // non-button element/component. Only a real <button> wants nativeButton=true —
  // a component child can't be introspected for its rendered tag, so we key off
  // identity: the kit Button is the one component known to render a native button.
  const childType = (children as React.ReactElement)?.type
  const nativeButton = hover
    ? false
    : typeof childType === 'string'
      ? childType === 'button'
      : childType === Button
  return (
    <Root open={isOpen} onOpenChange={setOpen}>
      <PopoverTrigger
        nativeButton={nativeButton}
        render={hover ? <span className="inline-block" {...hoverHandlers}>{children}</span> : (children as React.ReactElement)}
      />
      <PopoverContent side={side} align={align} className={className} {...hoverHandlers}>
        {title != null && <div className="mb-1 font-medium">{title}</div>}
        {content}
      </PopoverContent>
    </Root>
  )
}
