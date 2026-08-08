import * as React from 'react'
import {
  DropdownMenu as Root, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator, DropdownMenuLabel, DropdownMenuGroup,
} from '../shadcn/dropdown-menu'
import { ScrollArea } from './scroll-area'
import { Button } from './button'
import { cn } from '../lib/utils'

export type DropdownItem =
  | { type: 'divider' }
  | { type: 'label'; label: React.ReactNode }
  | {
      key: string
      label: React.ReactNode
      icon?: React.ReactNode
      onClick?: () => void
      danger?: boolean
      disabled?: boolean
    }

export interface DropdownProps {
  items: DropdownItem[]
  children: React.ReactElement
  side?: 'top' | 'right' | 'bottom' | 'left'
  align?: 'start' | 'center' | 'end'
  /** Collision handling for the align axis. Defaults to Base UI's `flip`, which
   *  swaps start↔end near a viewport edge; pass `{ align: 'shift' }` to keep the
   *  requested align and slide it into view instead. */
  collisionAvoidance?: React.ComponentProps<
    typeof DropdownMenuContent
  >['collisionAvoidance']
  /** Disables the trigger (legacy `disabled`). */
  disabled?: boolean
  /** Override the native-button heuristic below. Set `false` when the trigger is a
   *  component that renders a non-<button> element (e.g. kit <Tag>, a <span> pill) —
   *  Base UI then supplies button ARIA/keyboard semantics instead of warning that a
   *  real <button> was expected. Omit to auto-detect from the child element type. */
  nativeButton?: boolean
  /** Global selection handler receiving the activated item's `key` (legacy `menu.onClick`).
   *  Fires in addition to a per-item `onClick`. */
  onSelect?: (key: string) => void
  /** Controlled open state. Omit for the default uncontrolled behavior. */
  open?: boolean
  /** Fires when the menu requests an open-state change (pairs with `open`). */
  onOpenChange?: (open: boolean) => void
  /** Initial open state when uncontrolled (legacy `defaultOpen`). */
  defaultOpen?: boolean
  /** Test selector — forwarded onto the menu content <root>. Items derive `${testid}-item-${key}`. */
  'data-testid': string
}

export function Dropdown({ items, children, side, align = 'end', collisionAvoidance, disabled, onSelect, open, onOpenChange, defaultOpen, nativeButton: nativeButtonProp, 'data-testid': testid }: DropdownProps) {
  // Base UI's trigger defaults to `nativeButton: true` and warns if the rendered
  // element isn't a real <button>. Our trigger is a caller-supplied element that
  // may be a native <button>, the kit <Button> (which renders one), or a
  // non-button element/component — a bare <div role="button"> (legacy Radix
  // pattern) or a styled pill like kit <Tag> (renders a <span>). Only a real
  // <button> wants nativeButton=true; everything else needs nativeButton=false so
  // Base UI supplies the button ARIA/keyboard semantics on the non-button element.
  // A component child can't be introspected for its rendered tag, so we key off
  // identity: the kit Button is the one component known to render a native button.
  // A caller can still force the value with the `nativeButton` prop.
  const childType = (children as React.ReactElement)?.type
  const nativeButton =
    nativeButtonProp ??
    (typeof childType === 'string' ? childType === 'button' : childType === Button)
  return (
    <Root open={open} onOpenChange={onOpenChange} defaultOpen={defaultOpen}>
      <DropdownMenuTrigger render={children} disabled={disabled} nativeButton={nativeButton} />
      {/* The menu sizes to its widest item — that is now the vendored content's OWN default
          (`optionListPopupWidth`), so the `w-fit` that used to be stated here is gone: a local
          override of a rule that is no longer wrong is just a second place for it to drift.
          overflow-y-hidden max-h-none p-0: hand scrolling off to the ScrollArea
          (OverlayScrollbars) below, so a long menu uses the app's overlay
          scrollbar instead of the native one. The ScrollArea owns the
          available-height cap + the p-1 item padding. */}
      <DropdownMenuContent side={side} align={align} collisionAvoidance={collisionAvoidance} className="overflow-y-hidden max-h-none p-0" data-testid={testid}>
        <ScrollArea axis="y" autoHide="leave" className="max-h-(--available-height) p-1">
        {items.map((it, i) =>
          'type' in it && it.type === 'divider' ? (
            <DropdownMenuSeparator key={`d${i}`} />
          ) : 'type' in it && it.type === 'label' ? (
            // Base UI's GroupLabel requires a Group ancestor (throws
            // "MenuGroupContext is missing" otherwise), so wrap the section
            // label in its own group.
            <DropdownMenuGroup key={`l${i}`}>
              <DropdownMenuLabel>{it.label}</DropdownMenuLabel>
            </DropdownMenuGroup>
          ) : (
            <DropdownMenuItem
              key={(it as { key: string }).key}
              data-testid={testid ? `${testid}-item-${(it as { key: string }).key}` : undefined}
              disabled={(it as { disabled?: boolean }).disabled}
              onClick={() => {
                ;(it as { onClick?: () => void }).onClick?.()
                onSelect?.((it as { key: string }).key)
              }}
              className={cn((it as { danger?: boolean }).danger && 'text-destructive focus:text-destructive')}
            >
              {(it as { icon?: React.ReactNode }).icon != null && (
                <span aria-hidden>{(it as { icon?: React.ReactNode }).icon}</span>
              )}
              {(it as { label: React.ReactNode }).label}
            </DropdownMenuItem>
          ),
        )}
        </ScrollArea>
      </DropdownMenuContent>
    </Root>
  )
}
