import * as React from 'react'
import { CheckCircle2, AlertTriangle, XCircle, Info } from 'lucide-react'
import {
  AlertDialog, AlertDialogContent, AlertDialogHeader, AlertDialogFooter,
  AlertDialogTitle, AlertDialogDescription, AlertDialogAction, AlertDialogCancel,
} from '../shadcn/alert-dialog'
import { cn } from '../lib/utils'

// Imperative dialogs (shadcn has only declarative <Dialog>). Call `dialog.confirm(...)` from
// anywhere — event handlers, store actions, non-React code — and await the result. Backed by a
// singleton store + a single <DialogHost/> mounted once at the app root (same pattern as sonner's
// <Toaster/>). All visible/a11y strings are required (no defaults) so they stay translatable.
type Tone = 'default' | 'success' | 'warning' | 'error'
interface DialogItem {
  id: number
  title: React.ReactNode
  description?: React.ReactNode
  okText: string
  /** present → a Cancel button is shown (confirm); absent → single-OK alert. */
  cancelText?: string
  danger?: boolean
  tone?: Tone
  /** Optional stable test id stamped on the dialog content + OK/Cancel actions. */
  testid?: string
  /** Optional unique data-testid for the OK/confirm button (i18n-safe select). */
  okTestId?: string
  resolve: (ok: boolean) => void
}

let seq = 0
let items: DialogItem[] = []
const listeners = new Set<() => void>()
const emit = () => { items = [...items]; listeners.forEach((l) => l()) }
const subscribe = (l: () => void) => { listeners.add(l); return () => listeners.delete(l) }
const getSnapshot = () => items

function push(partial: Omit<DialogItem, 'id' | 'resolve'>): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    items = [...items, { ...partial, id: seq++, resolve }]
    listeners.forEach((l) => l())
  })
}

export interface ConfirmOptions {
  title: React.ReactNode
  description?: React.ReactNode
  okText: string
  cancelText: string
  danger?: boolean
  /** Optional stable test id stamped on the dialog content + OK/Cancel actions. */
  testid?: string
  /** Optional unique data-testid for the OK/confirm button (i18n-safe select). */
  okTestId?: string
}
export interface AlertOptions {
  title: React.ReactNode
  description?: React.ReactNode
  okText: string
  /** Optional stable test id stamped on the dialog content + OK action. */
  testid?: string
}

export const dialog = {
  /** Two-button prompt; resolves true on confirm, false on cancel/dismiss. */
  confirm: (o: ConfirmOptions) => push(o),
  /** Single-OK alerts; resolve when acknowledged. */
  info: (o: AlertOptions) => push({ ...o, tone: 'default' }).then(() => undefined),
  success: (o: AlertOptions) => push({ ...o, tone: 'success' }).then(() => undefined),
  warning: (o: AlertOptions) => push({ ...o, tone: 'warning' }).then(() => undefined),
  error: (o: AlertOptions) => push({ ...o, tone: 'error' }).then(() => undefined),
}

const toneIcon = { success: CheckCircle2, warning: AlertTriangle, error: XCircle, default: Info } as const
// Semantic status tokens (WCAG-AA on the dialog surface in both themes) — never
// raw palette classes like text-amber-600 (rgb 227,98,9 → 3.49:1 on white, fails AA).
const toneColor = { success: 'text-success', warning: 'text-warning', error: 'text-destructive', default: 'text-info' } as const

// Mount ONCE at the app root, alongside <Toaster/>.
export function DialogHost() {
  const list = React.useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
  const close = (it: DialogItem, ok: boolean) => {
    // settled-guard: AlertDialogAction/Cancel onClick AND Radix's onOpenChange both fire — only
    // the first (still-queued) call resolves; the rest are no-ops. Also serializes the queue.
    if (!items.some((x) => x.id === it.id)) return
    it.resolve(ok)
    items = items.filter((x) => x.id !== it.id)
    emit()
  }
  // Render only the HEAD of the queue → one focus trap / one overlay at a time; the next
  // item appears after the current resolves.
  const it = list[0]
  if (it == null) return null
  // an icon only for the single-OK alert variants (info/success/warning/error).
  const Icon = it.cancelText == null ? toneIcon[it.tone ?? 'default'] : null
  return (
    <AlertDialog key={it.id} open onOpenChange={(o) => { if (!o) close(it, false) }}>
      <AlertDialogContent data-testid={it.testid} {...(it.description == null ? { 'aria-describedby': undefined } : {})}>
        <AlertDialogHeader>
          <AlertDialogTitle className="flex items-center gap-2">
            {Icon != null && <Icon className={cn('size-5 shrink-0', toneColor[it.tone ?? 'default'])} aria-hidden />}
            {it.title}
          </AlertDialogTitle>
          {it.description != null && <AlertDialogDescription>{it.description}</AlertDialogDescription>}
        </AlertDialogHeader>
        <AlertDialogFooter>
          {it.cancelText != null && (
            <AlertDialogCancel data-testid={it.testid ? `${it.testid}-cancel-btn` : undefined} onClick={() => close(it, false)}>{it.cancelText}</AlertDialogCancel>
          )}
          <AlertDialogAction
            data-testid={it.testid ? `${it.testid}-ok-btn` : it.okTestId}
            onClick={() => close(it, true)}
            className={cn(it.danger && 'bg-destructive text-destructive-foreground hover:bg-destructive/90')}
          >
            {it.okText}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
