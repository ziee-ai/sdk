import type { ReactNode } from 'react'
import {
  RenderComponentLike,
  type ComponentLike,
} from '../lazy-component-view'

// @ziee/framework/router — self-contained lazy component renderer.
//
// Materializes a route element that may be an already-rendered element, a
// `React.lazy` component, or a bare dynamic-import loader
// (`() => import('./Page')`), and wraps lazy ones in `<Suspense>`. No `@/core`
// coupling — the fallback is passed in.
//
// The classification + Suspense-ownership logic lives in `../lazy-component-view`
// so this and `@ziee/shell`'s `LazyComponentRenderer` cannot drift apart again;
// they previously carried independent copies of a name-based loader heuristic
// and so carried the same silent-empty-region bug twice. See `../lazy-component`.

export function LazyRouteRenderer({
  component,
  fallback = null,
  debugId,
}: {
  component: ComponentLike
  fallback?: ReactNode
  /** Route path, for the dev diagnostics. */
  debugId?: string
}) {
  return (
    <RenderComponentLike
      component={component}
      fallback={fallback}
      debugId={debugId}
    />
  )
}
