import * as React from 'react'
import { CheckCircle2, Info, AlertTriangle, XCircle, X } from 'lucide-react'
import { Alert as Base, AlertTitle, AlertDescription } from '../shadcn/alert'
import { cn } from '../lib/utils'

// `neutral` is a non-semantic, muted-gray tone for informational states that
// must NOT read as success/warning/error — e.g. a user-cancelled action, which
// is a choice, not a failure.
export type AlertTone = 'info' | 'success' | 'warning' | 'error' | 'neutral'

const toneIcon: Record<AlertTone, React.ComponentType<{ className?: string }>> = {
  info: Info,
  success: CheckCircle2,
  warning: AlertTriangle,
  error: XCircle,
  neutral: Info,
}
// Semantic status tokens (dark-aware, AA as text on the page bg) — not raw
// palette hues, which failed WCAG AA contrast in dark mode.
const toneCls: Record<AlertTone, string> = {
  info: 'border-info/35 text-info [&>svg]:text-info',
  success: 'border-success/35 text-success [&>svg]:text-success',
  warning: 'border-warning/35 text-warning [&>svg]:text-warning',
  error: 'border-destructive/40 text-destructive [&>svg]:text-destructive',
  neutral: 'border-border text-muted-foreground [&>svg]:text-muted-foreground',
}

interface AlertCommon {
  tone?: AlertTone
  title?: React.ReactNode
  description?: React.ReactNode
  icon?: React.ReactNode
  className?: string
  children?: React.ReactNode
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// A dismissible alert MUST supply both onClose and an explicit closeLabel (no built-in
// default — the caller owns the string so it can be translated).
export type AlertProps =
  | (AlertCommon & { onClose?: undefined; closeLabel?: never })
  | (AlertCommon & { onClose: () => void; closeLabel: string })

export function Alert({ tone = 'info', title, description, icon, className, children, 'data-testid': testid, ...rest }: AlertProps) {
  const Icon = toneIcon[tone]
  const role = tone === 'error' || tone === 'warning' ? 'alert' : 'status'
  const onClose = (rest as { onClose?: () => void }).onClose
  const closeLabel = (rest as { closeLabel?: string }).closeLabel
  return (
    <Base role={role} className={cn(toneCls[tone], onClose && 'pe-10', 'relative', className)} data-testid={testid}>
      {/* Render the caller's icon as a DIRECT child (an <svg>), never wrapped in
          a <span>: the shadcn Alert base switches to its 2-column icon layout via
          `has-[>svg]` + `*:[svg]:row-span-2`, which only matches a direct svg. A
          wrapper span breaks that detection and the icon stacks ABOVE the title.
          We clone to force size-4 (so a bare lucide icon isn't its 24px default)
          + aria-hidden, keeping it a direct svg child. */}
      {icon != null ? (
        React.isValidElement<React.SVGProps<SVGSVGElement>>(icon) ? (
          React.cloneElement(icon, {
            'aria-hidden': true,
            className: cn('size-4', icon.props.className),
          })
        ) : (
          icon
        )
      ) : (
        <Icon className="size-4" aria-hidden />
      )}
      {title != null && <AlertTitle>{title}</AlertTitle>}
      {(description != null || children != null) && (
        <AlertDescription>{description}{children}</AlertDescription>
      )}
      {onClose && (
        <button
          type="button"
          onClick={onClose}
          aria-label={closeLabel}
          data-testid={`${testid}-close`}
          className="absolute end-2 top-2 rounded-sm p-1 text-foreground/60 hover:text-foreground hover:bg-foreground/10"
        >
          <X className="size-4" aria-hidden />
        </button>
      )}
    </Base>
  )
}
