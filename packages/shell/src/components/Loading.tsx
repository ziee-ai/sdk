import { Spin, Text } from '@ziee/kit'
import type { SpinProps } from '@ziee/kit'
import type { ReactNode } from 'react'

export interface LoadingProps extends Omit<SpinProps, 'children' | 'tip' | 'label'> {
  /** Fill the viewport height (min-h-screen) — for the app-bootstrap loader. */
  fullscreen?: boolean
  /** Optional label rendered under the spinner; also the spinner's accessible name. */
  tip?: ReactNode
  /** Accessible name override (defaults to the tip text, or "Loading"). */
  label?: string
  /** Extra classes on the centering wrapper. */
  className?: string
}

/**
 * Standard app loading indicator: a centered spinner with an optional label below it.
 *
 * Use this wherever a page / route / section / card is loading so the spinner
 * icon, size, and centering stay consistent across the whole UI. Defaults to
 * `size="lg"`; pass `size="sm"` for inline / widget spots, or `fullscreen`
 * for the app-bootstrap loader (AuthGuard). Callers need not pass a label —
 * the wrapper supplies a sensible default so existing call-sites stay valid.
 */
export function Loading({
  fullscreen = false,
  size = 'lg',
  tip,
  label,
  className = '',
  ...spin
}: LoadingProps) {
  const name = label ?? (typeof tip === 'string' ? tip : 'Loading')
  return (
    <div
      className={`flex flex-col items-center justify-center gap-3 ${
        fullscreen ? 'min-h-screen' : 'h-full w-full py-8'
      } ${className}`.trim()}
    >
      <Spin size={size} {...spin} label={name} />
      {tip != null && <Text type="secondary">{tip}</Text>}
    </div>
  )
}
