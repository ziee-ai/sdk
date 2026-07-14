import * as React from 'react'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

// SectionHeader — a title + right-aligned actions row with a HARD contract:
// **never-wrap-with-room**. The title and the actions stay on ONE row; when space
// is tight the TITLE truncates (min-w-0 + truncate) to make room, and the actions
// NEVER drop to a second line (shrink-0). This is the fix for the premature-stack
// bug (taxonomy B1): the kit Card header used `flex-col sm:flex-row`, which stacks
// a short title above a narrow `+` button on mobile even though both trivially fit
// on one row (the "Template Assistants / My Assistants / Configured providers"
// misses). Use this instead of `Card title=/extra=` for list/section headers.
export type SectionHeaderProps = Omit<
  React.ComponentProps<'div'>,
  'title' | 'style'
> & {
  title: React.ReactNode
  /** Right-aligned actions (buttons). Stay on the title's row; never wrap. */
  actions?: React.ReactNode
  /** Optional muted description under the title (also truncates). */
  description?: React.ReactNode
  /** Test selector — REQUIRED, forwarded onto the header root (i18n-safe). */
  'data-testid': string
} & KitStyleProps

export function SectionHeader({
  title,
  actions,
  description,
  className,
  style,
  allowStyle: _allowStyle,
  ...rest
}: SectionHeaderProps) {
  return (
    <div
      data-slot="section-header"
      style={style}
      className={cn('flex w-full flex-row items-center gap-3', className)}
      {...rest}
    >
      <div className="min-w-0 flex-1">
        <div
          data-slot="section-header-title"
          className="truncate text-base font-medium leading-snug"
        >
          {title}
        </div>
        {description != null && (
          <div className="truncate text-sm text-muted-foreground">
            {description}
          </div>
        )}
      </div>
      {actions != null && (
        <div
          data-slot="section-header-actions"
          className="flex shrink-0 items-center gap-2"
        >
          {actions}
        </div>
      )}
    </div>
  )
}
