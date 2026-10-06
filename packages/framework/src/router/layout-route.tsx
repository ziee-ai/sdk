import { Outlet } from 'react-router-dom'
import type { ReactNode } from 'react'
import { RenderComponentLike } from '../lazy-component-view'
import type { LayoutDefinition } from './types'

// @ziee/framework/router — how a LAYOUT becomes a route element.
//
// Split out of `RouterComponent` because both halves of it were wrong in the
// same way the slot/route renderers were (see `../lazy-component.ts`): they
// asked a component for its NAME and believed the answer.
//
//   key={layoutDef.component.name || 'layout'}
//
// For `React.lazy(() => import('./SiteLayout'))` the value is an exotic OBJECT,
// so `.name` is `undefined` and EVERY lazy layout collapsed onto the literal key
// `'layout'`. React then treats two structurally-adjacent `<Route>`s with the
// same key as the same element: the second layout group is reconciled INTO the
// first, so a route declared under the reader shell renders inside the site
// shell (and React logs a duplicate-key warning naming nothing useful). Making
// the five shells lazy — the whole point of code-splitting them — is what
// TRIGGERS the collision, which is why it was never seen with eager layouts.
//
// The layout was also mounted with a bare `<LayoutComponent>` and NO `Suspense`,
// so a lazy layout's suspension escaped to the app-level boundary and blanked
// the entire app for the duration of every layout switch.

/**
 * Identity, not name. A layout def is a module-scope object, so a WeakMap keyed
 * on it yields a key that is stable for the app's lifetime and distinct per
 * layout — the property `.name` was standing in for and getting wrong. An
 * explicit `id` on the def wins when present (readable in devtools, and stable
 * across a module reload that rebuilds the object).
 */
const assignedKeys = new WeakMap<LayoutDefinition, string>()
let sequence = 0

export function layoutRouteKey(layoutDef: LayoutDefinition): string {
  if (layoutDef.id) return `layout:${layoutDef.id}`
  let key = assignedKeys.get(layoutDef)
  if (!key) {
    key = `layout:${++sequence}`
    assignedKeys.set(layoutDef, key)
  }
  return key
}

/** TEST SEAM — reset the ordinal counter so key assertions are not order-coupled. */
export function __resetLayoutRouteKeys(): void {
  sequence = 0
}

/**
 * The element a layout group's parent `<Route>` renders. Goes through
 * `RenderComponentLike` for the same reason routes and slots do: the layout may
 * be a plain component, a `React.lazy` exotic, or a bare
 * `() => import('./SiteLayout')` loader, and only the renderer knows which of
 * those needs a `<Suspense>` boundary of its own.
 */
export function LayoutRouteElement({
  layoutDef,
  fallback,
  children = <Outlet />,
}: {
  layoutDef: LayoutDefinition
  fallback?: ReactNode
  children?: ReactNode
}) {
  return (
    <RenderComponentLike
      component={layoutDef.component}
      props={{ children }}
      fallback={fallback}
      debugId={layoutRouteKey(layoutDef)}
    />
  )
}
