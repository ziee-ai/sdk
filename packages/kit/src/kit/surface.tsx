import * as React from 'react'

/** Ambient "surface state" inherited by all kit components in a subtree.
 * One channel for every cross-cutting axis (add a field here, components opt in).
 * Components READ this and own their REACTION (disabled→disable, loading→their
 * own skeleton, size→density, readOnly→read-only render). */
export interface KitSurface {
  disabled?: boolean
  /** Data not ready — descendants render their own skeleton. */
  loading?: boolean
  readOnly?: boolean
  size?: 'sm' | 'default' | 'lg'
}

const SurfaceContext = React.createContext<KitSurface>({})

/** `over` wins per-axis when defined (incl. explicit `false`), else inherit. */
function merge(base: KitSurface, over: KitSurface): KitSurface {
  return {
    disabled: over.disabled ?? base.disabled,
    loading: over.loading ?? base.loading,
    readOnly: over.readOnly ?? base.readOnly,
    size: over.size ?? base.size,
  }
}

/** Set ambient surface for descendants. Nestable: merges with the parent surface,
 * so `<Provider loading>` inside `<Provider disabled>` yields { disabled, loading }.
 * Any container (Form, Card, Section, app root) uses THIS — never a bespoke context. */
export function KitSurfaceProvider({ children, ...value }: KitSurface & { children: React.ReactNode }) {
  const parent = React.useContext(SurfaceContext)
  const merged = React.useMemo(
    () => merge(parent, value),
    [parent, value.disabled, value.loading, value.readOnly, value.size],
  )
  return <SurfaceContext.Provider value={merged}>{children}</SurfaceContext.Provider>
}

/** Resolve the ambient surface against a component's own props (own wins when defined).
 * The single place precedence lives — every kit control calls this. */
export function useSurface(own: KitSurface = {}): KitSurface {
  return merge(React.useContext(SurfaceContext), own)
}

/** Sugar: a loading boundary — everything inside renders its own skeleton. */
export const Loading = ({ children }: { children: React.ReactNode }) => (
  <KitSurfaceProvider loading>{children}</KitSurfaceProvider>
)
