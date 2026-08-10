import { Component, type ReactNode, type ErrorInfo } from 'react'

interface Props {
  /** Render-prop fallback for when a child throws. Receives the caught error. */
  fallback: (error: Error, reset: () => void) => ReactNode
  /** Optional label used in `console.error` to identify which boundary caught the throw. */
  label?: string
  /** Optional `onError` side effect (telemetry, etc.). */
  onError?: (error: Error, info: ErrorInfo) => void
  /**
   * When any value here changes WHILE the boundary is showing its fallback, the
   * error state is cleared and the children get another render.
   *
   * An error boundary latches by design, so without a reset signal one crash is
   * permanent for the life of the tab — observed in production as a surface that
   * stayed dead across four subsequent navigations. `AppShell` passes a history
   * epoch, so navigating away gives the module a fresh attempt.
   *
   * The PROP is inert while there is no error: `componentDidUpdate` returns early
   * unless a fallback is showing, so a healthy subtree is never remounted or
   * re-rendered on account of this prop. Note this is a claim about the prop, NOT
   * about the caller: whatever the caller derives the key FROM (in `AppShell`, a
   * history epoch) does re-render the caller on change, and that cost is the
   * caller's to justify.
   */
  resetKeys?: readonly unknown[]
  children: ReactNode
}

interface State {
  error: Error | null
}

/** Shallow, length-aware comparison of two `resetKeys` arrays. */
function changed(
  a: readonly unknown[] | undefined,
  b: readonly unknown[] | undefined,
): boolean {
  if (a === b) return false
  if (!a || !b) return a !== b
  if (a.length !== b.length) return true
  return a.some((v, i) => !Object.is(v, b[i]))
}

/**
 * Hand-rolled error boundary. Avoids the `react-error-boundary` dep.
 *
 * Why two layers (top-level + per-module):
 *   - Top-level (app entry) prevents a render throw anywhere in the tree from
 *     showing a blank page (React 18+ unmounts the whole tree on uncaught
 *     render errors).
 *   - Per-module (AppShell, around each module component) isolates a single
 *     module's crash so the shell + other modules continue to work. Mirrors the
 *     plugin-architecture spirit of the module system.
 */
export class AppErrorBoundary extends Component<Props, State> {
  state: State = { error: null }

  static getDerivedStateFromError(error: Error): State {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const tag = this.props.label ? ` [${this.props.label}]` : ''
    // Logged UNCONDITIONALLY, including for a crash that a `resetKeys` change
    // later clears. Auto-reset must never make a crash invisible to the
    // runtime-health gate (which counts `[AppErrorBoundary…]` console errors) —
    // recovery is for the user, not for the metrics.
    console.error(`[AppErrorBoundary${tag}]`, error, info.componentStack)
    this.props.onError?.(error, info)
  }

  componentDidUpdate(prev: Props) {
    // Only act while actually showing a fallback: a healthy subtree must never be
    // disturbed by a resetKeys change.
    if (!this.state.error) return
    if (!changed(prev.resetKeys, this.props.resetKeys)) return
    this.setState({ error: null })
  }

  reset = () => {
    this.setState({ error: null })
  }

  render() {
    if (this.state.error) {
      return this.props.fallback(this.state.error, this.reset)
    }
    return this.props.children
  }
}
