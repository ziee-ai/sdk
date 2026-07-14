import {
  Suspense,
  lazy,
  isValidElement,
  useMemo,
  type ReactNode,
  type ComponentType,
  type ReactElement,
} from 'react'
import { Loading } from './Loading'

type LazyComponent = () => Promise<{ default: ComponentType<any> }>
type ComponentLike = ComponentType<any> | LazyComponent | ReactElement

/**
 * Global cache of `React.lazy()` wrappers keyed by the loader function.
 *
 * `lazy()` MUST be called once per loader for the lifetime of the app: every
 * call returns a fresh lazy component *type*, and React unmounts the old
 * subtree + mounts the new one whenever the type changes. Creating it inside a
 * render (or a `useMemo` whose deps churn — e.g. a defaulted `props = {}` that
 * is a new object each render) silently remounts the subtree on every parent
 * re-render. For the router that meant a new `<BrowserRouter>` (new history)
 * mid-boot, leaving `navigate()` bound to a dead history. Keying by the loader
 * identity makes the lazy type stable across renders.
 */
const lazyComponentCache = new WeakMap<LazyComponent, ComponentType<any>>()

/**
 * Stable default for the `props` param. A `props = {}` default literal is a
 * fresh object every render, which churns the `useMemo` deps below (and, before
 * the lazy-type cache, remounted the subtree). Sharing one frozen empty object
 * keeps the no-props case referentially stable.
 */
const EMPTY_PROPS: Record<string, any> = Object.freeze({})

function getCachedLazy(loader: LazyComponent): ComponentType<any> {
  let Lazy = lazyComponentCache.get(loader)
  if (!Lazy) {
    Lazy = lazy(loader)
    lazyComponentCache.set(loader, Lazy)
  }
  return Lazy
}

interface LazyComponentRendererProps {
  /**
   * Component to render - can be:
   * 1. Lazy-loaded function (from lazyWithPreload)
   * 2. Regular React component
   * 3. Already-rendered React element
   */
  component: ComponentLike

  /**
   * Props to pass to the component (ignored if component is ReactElement)
   */
  props?: Record<string, any>

  /**
   * Custom fallback to show while lazy loading
   * @default <Loading size="sm" />
   */
  fallback?: ReactNode
}

/**
 * Universal component renderer with automatic lazy loading support.
 *
 * Handles three component types:
 * 1. Lazy-loaded functions (from lazyWithPreload) - wrapped in Suspense
 * 2. Regular React components - rendered directly
 * 3. Already-rendered React elements - passed through as-is
 *
 * Automatically wraps lazy components in Suspense with a configurable fallback.
 *
 * @example
 * ```tsx
 * // Lazy component with default spinner
 * <LazyComponentRenderer component={LazyWidget} props={{ id: 1 }} />
 *
 * // Custom fallback
 * <LazyComponentRenderer component={LazyRoute} fallback={<Loading />} />
 *
 * // No fallback
 * <LazyComponentRenderer component={LazyApp} fallback={null} />
 * ```
 */
export function LazyComponentRenderer({
  component,
  props = EMPTY_PROPS,
  fallback = <Loading size="sm" />,
}: LazyComponentRendererProps) {
  // Check if it's a lazy function by checking if it's a function with 0 params
  // AND doesn't have React component characteristics (like a name or prototype properties)
  const isLikelyLazy =
    typeof component === 'function' &&
    component.length === 0 &&
    !component.prototype?.isReactComponent && // Not a class component
    (!component.name || component.name === '') // Lazy functions from lazyWithPreload are usually anonymous

  const renderedComponent = useMemo(() => {
    // If it's already a valid React element, return as-is
    if (isValidElement(component)) {
      return component
    }

    // If it looks like a lazy function, wrap with lazy. The lazy *type* is
    // cached by loader identity (see `getCachedLazy`) so it stays stable across
    // renders — recreating it would remount the whole subtree.
    if (isLikelyLazy) {
      const LazyComponent = getCachedLazy(component as LazyComponent)
      return <LazyComponent {...props} />
    }

    // Otherwise it's a regular component - render it directly
    const Component = component as ComponentType<any>
    return <Component {...props} />
  }, [component, props, isLikelyLazy])

  // Wrap in Suspense for lazy components
  if (isLikelyLazy) {
    return <Suspense fallback={fallback}>{renderedComponent}</Suspense>
  }

  return <>{renderedComponent}</>
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
