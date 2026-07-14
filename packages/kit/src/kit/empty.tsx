import * as React from 'react'
import { Empty as Base, EmptyHeader, EmptyMedia, EmptyTitle, EmptyDescription, EmptyContent } from '../shadcn/empty'

export interface EmptyProps {
  title?: React.ReactNode
  description?: React.ReactNode
  icon?: React.ReactNode
  /** Custom illustration (legacy `image`), shown larger than `icon`. Takes precedence over icon. */
  image?: React.ReactNode
  /** Action(s) — e.g. a "Create" button. */
  children?: React.ReactNode
  className?: string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}

export function Empty({ title, description, icon, image, children, className, 'data-testid': testid }: EmptyProps) {
  return (
    <Base className={className} data-testid={testid}>
      <EmptyHeader>
        {image != null
          ? <EmptyMedia variant="default" aria-hidden>{image}</EmptyMedia>
          : icon != null && <EmptyMedia variant="icon" aria-hidden>{icon}</EmptyMedia>}
        {title != null && <EmptyTitle>{title}</EmptyTitle>}
        {description != null && <EmptyDescription>{description}</EmptyDescription>}
      </EmptyHeader>
      {children != null && <EmptyContent>{children}</EmptyContent>}
    </Base>
  )
}
