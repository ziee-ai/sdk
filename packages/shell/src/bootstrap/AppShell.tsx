import { useEffect, useMemo, useState } from 'react'
import type { ComponentRegistration } from '@ziee/framework/module-system/types'
import { ModuleSystem } from '@ziee/framework/stores'
import { initSync } from '@ziee/framework/sync'
import { AppErrorBoundary } from '../error/AppErrorBoundary'
import { ModuleErrorFallback } from '../error/ModuleErrorFallback'
import { useHistoryEpoch } from '../hooks/useHistoryEpoch'
import { ThemeProvider } from '../theme/ThemeProvider'
import { LazyComponentRenderer } from '../components/LazyComponentRenderer'
import { usePrefetchModules } from '../hooks/usePrefetchModules'

/**
 * ConditionalComponent — wrapper that checks a module component's `shouldMount`
 * hook before rendering.
 */
function ConditionalComponent({
  registration,
}: {
  registration: ComponentRegistration
}) {
  // Call shouldMount hook if provided, default to true
  const shouldMount = registration.shouldMount?.() ?? true

  // Memoize the component renderer to prevent recreating it when shouldMount changes
  const renderer = useMemo(
    () => (
      <LazyComponentRenderer
        component={registration.component}
        fallback={null}
        debugId={`module component '${registration.id}'`}
      />
    ),
    [registration.component, registration.id],
  )

  if (!shouldMount) {
    return null
  }

  return renderer
}

export interface AppShellProps {
  /**
   * The app's auth store (a zustand store exposing the auth session). Wired to
   * the realtime-sync SSE lifecycle via `initSync` — idempotent; starts on
   * login, stops on logout, restarts on user switch.
   *
   * SEAM: the app owns module discovery (`loadModules()`) — call it at the app
   * entry BEFORE mounting `<AppShell>`. AppShell only renders whatever modules
   * registered components (sorted by `order`).
   */
  authStore: Parameters<typeof initSync>[0]

  /**
   * Resolves once the app has discovered + registered all modules. When the app
   * loads modules via a LAZY glob (each module.tsx its own chunk, kept out of
   * the entry chunk), registration is async — the shell must wait for it before
   * rendering, because the router component, the theme store (ConfigClient), and
   * every slot come from module registration. If omitted, the shell renders
   * immediately (for apps that register modules synchronously before mount).
   */
  modulesReady?: Promise<void>
}

/**
 * AppShell — the generic, slot-driven application body.
 *
 * Renders every module-registered component (sorted by `order`, so the router's
 * order-0 component mounts first) inside a `ThemeProvider`, each wrapped in its
 * own error boundary so a single module crash isolates to that module — the
 * shell + other modules keep working. The outer boundary at the app entry
 * catches anything that escapes this layer (e.g. a ThemeProvider throw).
 *
 * The shell hard-codes NO pages or modules: an app fills the slots (sidebar,
 * settings, routes) via the module system, and supplies its own branding/nav.
 */
export function AppShell({ authStore, modulesReady }: AppShellProps) {
  // Gate rendering until modules are registered (async lazy-glob discovery).
  // Until then the router / theme store / slots don't exist yet, so we render a
  // minimal, theme-independent placeholder. `true` immediately when no promise
  // is supplied (synchronous-registration apps keep the old zero-flash boot).
  const [ready, setReady] = useState(!modulesReady)
  useEffect(() => {
    if (!modulesReady) return
    let active = true
    void modulesReady.then(() => {
      if (active) setReady(true)
    })
    return () => {
      active = false
    }
  }, [modulesReady])

  // Wire the realtime-sync SSE stream to the auth lifecycle.
  useEffect(() => {
    initSync(authStore)
  }, [authStore])

  const { components } = ModuleSystem

  // Sort components by order (the router component has order: 0, so it renders first)
  const sortedComponents = useMemo(() => {
    return [...components].sort((a, b) => (a.order ?? 0) - (b.order ?? 0))
  }, [components])

  if (!ready) {
    // Modules not registered yet — theme store isn't available, so avoid
    // ThemeProvider and render a bare, self-contained boot surface.
    return <div data-testid="app-root" className="h-full" />
  }

  return <AppShellBody sortedComponents={sortedComponents} />
}

/**
 * The real shell body, mounted only once modules are registered so the theme
 * store + prefetch seam are guaranteed available.
 */
function AppShellBody({
  sortedComponents,
}: {
  sortedComponents: ComponentRegistration[]
}) {
  // Prefetch lazy-loaded modules when the browser is idle
  usePrefetchModules()

  // Navigating is the "give it another go" signal for a latched boundary. Read
  // above the router (which is itself one of the module components below), so
  // `useLocation` is not available here — see `useHistoryEpoch`.
  const historyEpoch = useHistoryEpoch()

  return (
    <ThemeProvider>
      <div data-testid="app-root" className="h-full">
        {sortedComponents.map(comp => (
          <AppErrorBoundary
            key={comp.id}
            label={comp.id}
            resetKeys={[historyEpoch]}
            // A caught module crash renders a VISIBLE, actionable surface — not
            // `null`. `null` was the white-screen bug: the ROUTER is a module
            // component too, and it renders the entire routed app, so swallowing
            // its crash emptied the document with no message and no way back.
            // Keeping the fallback per-module preserves the isolation half of the
            // contract: a non-router module failing still leaves the shell and
            // every sibling module rendering normally.
            fallback={(error, reset) => (
              <ModuleErrorFallback
                error={error}
                reset={reset}
                moduleId={comp.id}
              />
            )}
          >
            <ConditionalComponent registration={comp} />
          </AppErrorBoundary>
        ))}
      </div>
    </ThemeProvider>
  )
}
