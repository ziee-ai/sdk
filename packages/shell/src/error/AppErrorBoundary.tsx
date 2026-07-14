import { Component, type ReactNode, type ErrorInfo } from 'react'

interface Props {
  /** Render-prop fallback for when a child throws. Receives the caught error. */
  fallback: (error: Error, reset: () => void) => ReactNode
  /** Optional label used in `console.error` to identify which boundary caught the throw. */
  label?: string
  /** Optional `onError` side effect (telemetry, etc.). */
  onError?: (error: Error, info: ErrorInfo) => void
  children: ReactNode
}

interface State {
  error: Error | null
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
    console.error(`[AppErrorBoundary${tag}]`, error, info.componentStack)
    this.props.onError?.(error, info)
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
