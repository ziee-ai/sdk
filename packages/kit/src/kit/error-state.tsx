import * as React from 'react'
import { Alert } from './alert'
import { Button } from './button'
import { cn } from '../lib/utils'

export interface ErrorStateProps {
  /**
   * Names the resource that failed to load; the title renders
   * "Couldn't load {resource}". REQUIRED — the error must always name the
   * resource, never a bare generic string.
   */
  resource: string
  /**
   * Optional HUMAN one-liner shown under the title. NEVER raw error text,
   * transport state ("SSE disconnected; reconnecting…"), or a placeholder —
   * technical detail belongs in `details` (behind the disclosure).
   */
  description?: React.ReactNode
  /**
   * Technical detail (raw error string, transport state) revealed only behind
   * a "Details" disclosure — never rendered inline as the description.
   */
  details?: React.ReactNode
  /**
   * Re-invokes the section's fetch. Renders the "Try again" button. Omit only
   * when no re-fetch is meaningful.
   */
  onRetry?: () => void
  /** Retry button label. Default "Try again". */
  retryLabel?: string
  /**
   * `inline` (fills a card section — default) or `page` (centered in an
   * otherwise-empty route, e.g. a single-source page).
   */
  variant?: 'inline' | 'page'
  className?: string
  /** Test selector — REQUIRED, forwarded onto the alert root (i18n-safe). */
  'data-testid': string
}

/**
 * The single shared error-state treatment for a failed data load. Replaces
 * every ad-hoc treatment (silent render, toast-only, stuck spinner, leaked dev
 * string). A destructive Alert that always names the resource, offers a human
 * one-liner, a "Try again" action, and a "Details" disclosure for the raw
 * error — never the raw error inline.
 *
 * Placement rules (enforced by callers):
 *  - Scope to the data: one ErrorState per independently-fetched resource.
 *  - Replace, don't stack: it takes the place of the section's
 *    content/spinner/empty-state — never an error banner + empty placeholder.
 *  - Never leave a spinner spinning on error.
 */
export function ErrorState({
  resource,
  description,
  details,
  onRetry,
  retryLabel = 'Try again',
  variant = 'inline',
  className,
  'data-testid': testid,
}: ErrorStateProps) {
  const [showDetails, setShowDetails] = React.useState(false)
  const hasActions = onRetry != null || details != null

  const alert = (
    <Alert
      tone="error"
      data-testid={testid}
      title={`Couldn't load ${resource}`}
      description={description}
      className={cn('w-full', variant === 'inline' ? className : undefined)}
    >
      {hasActions && (
        <div className="mt-3 flex flex-wrap items-center gap-2">
          {onRetry != null && (
            <Button
              variant="outline"
              onClick={onRetry}
              data-testid={`${testid}-retry`}
            >
              {retryLabel}
            </Button>
          )}
          {details != null && (
            <Button
              variant="ghost"
              onClick={() => setShowDetails((v) => !v)}
              aria-expanded={showDetails}
              data-testid={`${testid}-details-toggle`}
            >
              {showDetails ? 'Hide details' : 'Details'}
            </Button>
          )}
        </div>
      )}
      {details != null && showDetails && (
        <pre
          data-testid={`${testid}-details`}
          className="mt-2 w-full max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-sm bg-muted p-2 text-xs text-muted-foreground"
        >
          {details}
        </pre>
      )}
    </Alert>
  )

  if (variant === 'page') {
    return (
      <div
        data-testid={`${testid}-page`}
        className={cn(
          'flex min-h-60 w-full items-center justify-center p-6',
          className,
        )}
      >
        <div className="w-full max-w-md">{alert}</div>
      </div>
    )
  }

  return alert
}
