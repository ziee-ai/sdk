import type { ReactNode } from 'react'
import { RenderComponentLike, type ComponentLike } from '@ziee/framework'
import { Loading } from './Loading'

interface LazyComponentRendererProps {
  /**
   * Component to render - can be:
   * 1. A dynamic-import loader (`() => import('./Widget')`)
   * 2. A `React.lazy` component
   * 3. A regular React component
   * 4. An already-rendered React element
   */
  component: ComponentLike

  /**
   * Props to pass to the component (ignored if component is a ReactElement)
   */
  props?: Record<string, any>

  /**
   * Custom fallback to show while lazy loading
   * @default <Loading size="sm" />
   */
  fallback?: ReactNode

  /**
   * Slot key / banner id / registration id. Purely diagnostic: it is what turns
   * "some region of the shell is empty" into a message naming the entry at
   * fault. Pass it wherever the caller knows the id.
   */
  debugId?: string
}

/**
 * Universal component renderer with automatic lazy loading support.
 *
 * The classification (which of the four kinds is this?) and the `<Suspense>`
 * ownership live in `@ziee/framework`'s `RenderComponentLike`, shared with
 * `@ziee/framework/router`'s `LazyRouteRenderer`. They used to be two
 * independent copies of a heuristic that asked whether the function was
 * ANONYMOUS — so the documented `{ component: () => import('./X') }` shape
 * (whose arrow is named `component`, because JS infers a name from the property
 * it initializes) was classified as a plain component, invoked, and returned a
 * Promise React could never settle. The slot rendered nothing, forever, with no
 * error. See `@ziee/framework/lazy-component` for the shape/marker rules that
 * replaced it.
 *
 * @example
 * ```tsx
 * <LazyComponentRenderer component={() => import('./Widget')} props={{ id: 1 }} />
 * <LazyComponentRenderer component={LazyRoute} fallback={<Loading />} />
 * <LazyComponentRenderer component={LazyApp} fallback={null} />
 * ```
 */
export function LazyComponentRenderer({
  component,
  props,
  fallback = <Loading size="sm" />,
  debugId,
}: LazyComponentRendererProps) {
  return (
    <RenderComponentLike
      component={component}
      props={props}
      fallback={fallback}
      debugId={debugId}
    />
  )
}

// Legacy export for backwards compatibility
/**
 * @deprecated Use LazyComponentRenderer instead
 */
export function WidgetRenderer({
  widget,
  props,
}: {
  widget: { component: ComponentLike }
  props?: Record<string, any>
}) {
  return <LazyComponentRenderer component={widget.component} props={props} />
}
