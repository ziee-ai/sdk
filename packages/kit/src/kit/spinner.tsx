import * as React from 'react'
import { Spinner as Base } from '../shadcn/spinner'
import { cn } from '../lib/utils'

const sizes = { sm: 'size-4', default: 'size-5', lg: 'size-8' } as const

export interface SpinnerProps extends Omit<React.ComponentProps<'svg'>, 'size'> {
  size?: keyof typeof sizes
  /** Required accessible label for the loading state (no default — caller owns it for i18n). */
  label: string
}

export function Spinner({ size = 'default', label, className, ...props }: SpinnerProps) {
  return (
    <span role="status" aria-label={label} className="inline-flex">
      <Base className={cn(sizes[size], className)} aria-hidden {...props} />
    </span>
  )
}

export interface SpinProps {
  spinning?: boolean
  size?: keyof typeof sizes
  /** Required accessible label for the loading state (no default — caller owns it for i18n). */
  label: string
  /** Visible caption under the spinner (legacy `tip`/`description`). */
  description?: React.ReactNode
  children?: React.ReactNode
  className?: string
  'data-testid'?: string
}

/** Overlays a spinner on its children while `spinning` (bare spinner with no children). */
export function Spin({ spinning = true, size, label, description, children, className, 'data-testid': testid }: SpinProps) {
  if (children === undefined) {
    return (
      <div data-testid={testid} className={cn('inline-flex flex-col items-center gap-2', className)}>
        <Spinner size={size} label={label} />
        {description != null && <span className="text-sm text-muted-foreground">{description}</span>}
      </div>
    )
  }
  return (
    <div className={cn('relative', className)} aria-busy={spinning || undefined}>
      {children}
      {spinning && (
        <div className="absolute inset-0 z-10 grid place-items-center gap-2 bg-background/60 backdrop-blur-[1px]">
          <Spinner size={size} label={label} />
          {description != null && <span className="text-sm text-muted-foreground">{description}</span>}
        </div>
      )}
    </div>
  )
}
