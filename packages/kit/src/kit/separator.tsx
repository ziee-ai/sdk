import * as React from 'react'
import { Separator as Base } from '../shadcn/separator'
import { cn } from '../lib/utils'

export interface SeparatorProps {
  orientation?: 'horizontal' | 'vertical'
  children?: React.ReactNode
  /** Position of the label text on a labeled divider (legacy `titlePlacement`). Default 'center'. */
  titlePlacement?: 'left' | 'center' | 'right'
  className?: string
}

export function Separator({ orientation = 'horizontal', children, titlePlacement = 'center', className }: SeparatorProps) {
  if (children) {
    // labeled divider: the inner <Base> elements carry the separator role; the wrapper
    // must NOT also be role="separator" (a separator can't contain meaningful text).
    // titlePlacement shrinks the leading/trailing line so the label sits left/right/center.
    // Plain hairline <div>s for the lines (NOT the shadcn <Separator>, which
    // bakes in `shrink-0 w-full` — that fights `flex-1`, forcing both lines to
    // 100% width so the label overflows outside its container).
    const lead = titlePlacement === 'left' ? 'w-4 flex-none' : 'flex-1'
    const trail = titlePlacement === 'right' ? 'w-4 flex-none' : 'flex-1'
    return (
      <div className={cn('flex items-center gap-3 text-sm text-muted-foreground', className)}>
        <div role="separator" aria-orientation="horizontal" className={cn('h-px bg-border', lead)} />
        <span className="shrink-0">{children}</span>
        <div aria-hidden className={cn('h-px bg-border', trail)} />
      </div>
    )
  }
  return <Base orientation={orientation} className={className} />
}
