/**
 * ModuleErrorFallback — what a caught MODULE crash renders.
 *
 * ## Why this exists
 *
 * `AppShell` wraps every module-registered component in its own
 * `AppErrorBoundary` so one module's crash cannot take the app down. That
 * boundary's fallback used to be `() => null`, which realized the OPPOSITE of the
 * boundary doc-block's stated contract:
 *
 *   "Top-level (app entry) prevents a render throw anywhere in the tree from
 *    showing a blank page … Per-module (AppShell, around each module component)
 *    isolates a single module's crash so the shell + other modules continue to
 *    work."
 *
 * For a small module, `null` is a defensible "hide the broken widget". But the
 * ROUTER is a module component too, and it renders the entire routed app — nav,
 * layout, page. So a render throw anywhere under the router was caught here and
 * replaced with NOTHING: `document.body.innerText` went to zero length, no error
 * message, no affordance, and (because a boundary latches its error state) it
 * stayed that way until a manual reload. That is the white screen this file
 * removes; the app-entry boundary in `main.tsx` never got a chance to honour its
 * own no-blank-page promise, because this inner boundary caught first.
 *
 * ## Design notes
 *
 * - **Self-contained inline styles, no token classes.** Mirrors the app-entry
 *   fallback in the consuming app's `main.tsx`. A crash may itself be a
 *   theme/CSS/token-pipeline failure, so the surface that reports it must not
 *   depend on that pipeline to be legible.
 * - **`role="alert"`** so assistive tech announces it, and so tests can find it
 *   semantically rather than by class.
 * - **Two affordances, no automatic recovery.** "Try again" re-renders the module
 *   (a transient failure — a chunk blip, a race — clears); "Reload page" is the
 *   escape hatch. It deliberately does NOT auto-reload: the codebase's
 *   `chunk-recovery` module already settled this ("An automatic `location.reload()`
 *   during a chat session destroys the unsent draft in the composer and tears down
 *   an in-flight assistant stream, and it can loop … renders a 'Reload page'
 *   BUTTON and lets the user choose").
 * - It stays COMPACT and in-flow, so a non-router module failing still leaves the
 *   rest of the shell usable — the isolation half of the contract.
 */
import type { ReactNode } from 'react'

export interface ModuleErrorFallbackProps {
  /** The caught error. */
  error: Error
  /** Clears the boundary's error state and re-renders the module subtree. */
  reset: () => void
  /** The failing module's registration id, used only to label the surface. */
  moduleId?: string
}

export function ModuleErrorFallback({
  error,
  reset,
  moduleId,
}: ModuleErrorFallbackProps): ReactNode {
  return (
    <div
      role="alert"
      data-testid="module-error-fallback"
      data-module-id={moduleId}
      // bootstrap/crash-fallback: self-contained inline colors, must not depend
      // on the token CSS pipeline (the crash may BE a theme failure).
      data-allow-custom-color
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'flex-start',
        gap: 12,
        margin: 16,
        padding: 16,
        border: '1px solid #d9534f',
        borderRadius: 8,
        background: '#fff5f5',
        color: '#1a1a1a',
        fontFamily:
          '-apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
        maxWidth: 720,
      }}
    >
      <strong style={{ fontSize: 16 }}>Something went wrong</strong>
      <span data-allow-custom-color style={{ color: '#555', fontSize: 14 }}>
        This part of the app failed to render. You can try again, or reload the
        page if the problem persists.
      </span>
      <pre
        data-testid="module-error-fallback-message"
        data-allow-custom-color
        style={{
          margin: 0,
          padding: 8,
          width: '100%',
          overflowX: 'auto',
          background: '#f2f2f2',
          borderRadius: 4,
          fontSize: 12,
          whiteSpace: 'pre-wrap',
        }}
      >
        {error?.message ?? String(error)}
      </pre>
      <div style={{ display: 'flex', gap: 8 }}>
        <button
          type="button"
          data-testid="module-error-fallback-retry"
          data-allow-custom-color
          onClick={reset}
          style={{
            padding: '6px 12px',
            borderRadius: 6,
            border: '1px solid #999',
            background: '#fff',
            cursor: 'pointer',
            fontSize: 14,
          }}
        >
          Try again
        </button>
        <button
          type="button"
          data-testid="module-error-fallback-reload"
          data-allow-custom-color
          onClick={() => window.location.reload()}
          style={{
            padding: '6px 12px',
            borderRadius: 6,
            border: '1px solid #999',
            background: '#fff',
            cursor: 'pointer',
            fontSize: 14,
          }}
        >
          Reload page
        </button>
      </div>
    </div>
  )
}
