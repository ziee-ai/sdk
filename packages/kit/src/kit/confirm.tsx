import * as React from 'react'
import {
  AlertDialog as Root, AlertDialogTrigger, AlertDialogContent, AlertDialogHeader, AlertDialogFooter,
  AlertDialogTitle, AlertDialogDescription, AlertDialogCancel,
} from '../shadcn/alert-dialog'
import { Button } from './button'
import { useControllableState } from './use-controllable-state'

export interface ConfirmProps {
  title: React.ReactNode
  description?: React.ReactNode
  /** Confirm button text — required (no default, so it's always translatable). */
  okText: string
  /** Cancel button text — required (no default, so it's always translatable). */
  cancelText: string
  danger?: boolean
  onConfirm: () => void | Promise<void>
  /** Called when the user cancels/dismisses (legacy `onCancel`). */
  onCancel?: () => void
  /** Extra props forwarded to the confirm button (legacy `okButtonProps`), e.g. { danger: true }. */
  okButtonProps?: { danger?: boolean; disabled?: boolean }
  /** Controlled open state. Pair with `onOpenChange`; omit `children` for trigger-less use. */
  open?: boolean
  /** Fires when the open state should change (pairs with `open`). */
  onOpenChange?: (open: boolean) => void
  /** The trigger element. Optional when driving the dialog via `open`/`onOpenChange`. */
  children?: React.ReactElement
  /** Test selector — forwarded onto the dialog content <root> (i18n-safe). */
  'data-testid': string
}

// Built on AlertDialog (modal + focus-trapped + focus-restoring), not a Popover — an
// "are you sure?" prompt must trap focus and inert the background.
export function Confirm({ title, description, okText, cancelText, danger, onConfirm, onCancel, okButtonProps, open, onOpenChange, children, 'data-testid': testid }: ConfirmProps) {
  // Controllable: caller may drive `open` (trigger-less) or let the trigger own it.
  const [isOpen, setOpen] = useControllableState<boolean>({
    value: open, defaultValue: false, onChange: onOpenChange,
  })
  const [busy, setBusy] = React.useState(false)
  const run = async () => {
    setBusy(true)
    try {
      await onConfirm()
      setOpen(false)
    } catch {
      // keep the dialog open so the user can retry; caller surfaces the error.
    } finally {
      setBusy(false)
    }
  }
  const isDanger = danger || okButtonProps?.danger
  return (
    <Root open={isOpen} onOpenChange={(o) => { setOpen(o); if (!o) onCancel?.() }}>
      {children != null && <AlertDialogTrigger render={children} />}
      {/* suppress Radix's missing-description warning when intentionally absent */}
      <AlertDialogContent data-testid={testid} {...(description == null ? { 'aria-describedby': undefined } : {})}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          {description != null && <AlertDialogDescription>{description}</AlertDialogDescription>}
        </AlertDialogHeader>
        <AlertDialogFooter>
          {/* onCancel is fired once, from onOpenChange(false) — which also covers Esc + overlay. */}
          <AlertDialogCancel data-testid={`${testid}-cancel`} disabled={busy}>{cancelText}</AlertDialogCancel>
          {/* a plain Button (not AlertDialogAction) so the dialog only closes on success. */}
          <Button data-testid={`${testid}-confirm`} variant={isDanger ? 'destructive' : 'default'} disabled={okButtonProps?.disabled} loading={busy} onClick={run}>{okText}</Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </Root>
  )
}
