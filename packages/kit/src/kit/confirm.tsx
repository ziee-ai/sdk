import * as React from 'react'
import {
  AlertDialog as Root, AlertDialogTrigger, AlertDialogContent, AlertDialogHeader, AlertDialogFooter,
  AlertDialogTitle, AlertDialogDescription, AlertDialogCancel,
} from '../shadcn/alert-dialog'
import { Button } from './button'
import { useControllableState } from './use-controllable-state'
import { useInitialFocus, type InitialFocus } from '../internal/initial-focus'

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
  /**
   * Extra props forwarded to the confirm button (legacy `okButtonProps`), e.g. { danger: true }.
   *
   * `disabled` here is the LAST reason-less refusal left in this component, and it is kept only
   * for the case it was written for: a caller with genuinely nothing to say. When there IS a
   * reason, pass `okUnavailableReason` — it renders the confirm `aria-disabled` instead of natively
   * disabled, so the reason is reachable by hover AND by assistive tech (kit Button's
   * `unavailableReason`). A reason supplied there wins over this flag.
   */
  okButtonProps?: { danger?: boolean; disabled?: boolean }
  /** WHY the confirm cannot be pressed. Presence means unavailable; the string IS the reason. */
  okUnavailableReason?: string
  /** Controlled open state. Pair with `onOpenChange`; omit `children` for trigger-less use. */
  open?: boolean
  /** Fires when the open state should change (pairs with `open`). */
  onOpenChange?: (open: boolean) => void
  /** The trigger element. Optional when driving the dialog via `open`/`onOpenChange`. */
  children?: React.ReactElement
  /** Where focus lands when the prompt opens. Defaults to the first tabbable that is not
   *  destructive — which, for an "are you sure?", is Cancel. */
  initialFocus?: InitialFocus
  /** Test selector — forwarded onto the dialog content <root> (i18n-safe). */
  'data-testid': string
}

// Built on AlertDialog (modal + focus-trapped + focus-restoring), not a Popover — an
// "are you sure?" prompt must trap focus and inert the background.
export function Confirm({ title, description, okText, cancelText, danger, onConfirm, onCancel, okButtonProps, okUnavailableReason, open, onOpenChange, children, initialFocus, 'data-testid': testid }: ConfirmProps) {
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
  const popupRef = React.useRef<HTMLDivElement>(null)
  const resolveInitialFocus = useInitialFocus(popupRef, initialFocus)
  // A caller-supplied reason wins over the flag; while a confirm is in flight the reason is the
  // flight itself. Either way this is `unavailableReason`, never `disabled`.
  const okReason = okUnavailableReason ?? (okButtonProps?.disabled === true ? undefined : undefined)
  return (
    <Root open={isOpen} onOpenChange={(o) => { setOpen(o); if (!o) onCancel?.() }}>
      {children != null && <AlertDialogTrigger render={children} />}
      {/* suppress Radix's missing-description warning when intentionally absent */}
      <AlertDialogContent ref={popupRef} initialFocus={resolveInitialFocus} data-testid={testid} {...(description == null ? { 'aria-describedby': undefined } : {})}>
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          {description != null && <AlertDialogDescription>{description}</AlertDialogDescription>}
        </AlertDialogHeader>
        <AlertDialogFooter>
          {/* onCancel is fired once, from onOpenChange(false) — which also covers Esc + overlay.
              WHILE BUSY THIS IS `unavailableReason`, NOT `disabled`. A natively disabled Cancel is
              pointer-events:none and out of the tab order, so the one control a user reaches for
              when a confirm is taking too long went silent AND unfocusable — while Esc and the
              backdrop still dismissed, i.e. the refusal was not even true of the dialog, only of
              this button. Now it stays hoverable and focusable and says what it is waiting for. */}
          <AlertDialogCancel
            render={
              <Button
                variant="outline"
                data-testid={`${testid}-cancel`}
                unavailableReason={busy ? 'Waiting for the current action to finish' : undefined}
              >
                {cancelText}
              </Button>
            }
          />
          {/* a plain Button (not AlertDialogAction) so the dialog only closes on success. */}
          <Button
            data-testid={`${testid}-confirm`}
            variant={isDanger ? 'destructive' : 'default'}
            disabled={okReason == null && okButtonProps?.disabled}
            unavailableReason={okReason}
            loading={busy}
            onClick={run}
          >
            {okText}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </Root>
  )
}
