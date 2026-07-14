import * as React from 'react'
import { CheckCircle2, XCircle, AlertTriangle, Info } from 'lucide-react'
import { cn } from '../lib/utils'

// legacy Result → status page. Accepts semantic statuses AND HTTP-code statuses
// ('403'|'404'|'500') exactly as legacy does (the app uses status="403" etc.).
export type ResultStatus = 'success' | 'error' | 'warning' | 'info' | '403' | '404' | '500'
// HTTP codes map onto a semantic look: 404→info, 403→warning, 500→error.
const codeAlias: Record<string, 'success' | 'error' | 'warning' | 'info'> = {
  '403': 'warning', '404': 'info', '500': 'error',
}
const icons = { success: CheckCircle2, error: XCircle, warning: AlertTriangle, info: Info }
// Semantic status tokens (WCAG-AA in both themes) — never raw palette classes.
const colors = { success: 'text-success', error: 'text-destructive', warning: 'text-warning', info: 'text-info' }

export interface ResultProps {
  status?: ResultStatus
  title: React.ReactNode
  subtitle?: React.ReactNode
  icon?: React.ReactNode
  extra?: React.ReactNode
  className?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}

export function Result({ status = 'info', title, subtitle, icon, extra, className, 'data-testid': testid }: ResultProps) {
  const semantic = codeAlias[status] ?? (status as 'success' | 'error' | 'warning' | 'info')
  const Icon = icons[semantic]
  const role = semantic === 'error' || semantic === 'warning' ? 'alert' : 'status'
  return (
    <div role={role} className={cn('flex flex-col items-center gap-2 px-6 py-12 text-center', className)} data-testid={testid}>
      {icon ?? <Icon className={cn('size-12', colors[semantic])} aria-hidden />}
      <div className="text-lg font-semibold">{title}</div>
      {subtitle != null && <div className="max-w-md text-sm text-muted-foreground">{subtitle}</div>}
      {extra != null && <div className="mt-4 flex gap-2">{extra}</div>}
    </div>
  )
}
