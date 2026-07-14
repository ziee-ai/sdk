import {
  Suspense,
  lazy,
  isValidElement,
  useMemo,
  type ReactNode,
  type ComponentType,
} from 'react'

// @ziee/framework/router — self-contained lazy component renderer.
//
// A dependency-free port of ziee's `LazyComponentRenderer`: materializes a
// route element that may be an already-rendered element, a `React.lazy`
// component, or a bare dynamic-import loader (`() => import('./Page')`), and
// wraps lazy ones in `<Suspense>`. No `@/core` coupling — the fallback is
// passed in.

type LazyLoader = () => Promise<{ default: ComponentType<any> }>
type ComponentLike = ComponentType<any> | LazyLoader | ReactNode

/**
 * `lazy()` MUST be called once per loader for the app's lifetime — every call
 * returns a fresh lazy *type*, and React remounts the subtree when the type
 * changes. Keying by loader identity keeps the type stable across renders (a
 * fresh `<BrowserRouter>` mid-boot would otherwise strand `navigate()` on a
 * dead history).
 */
const lazyCache = new WeakMap<LazyLoader, ComponentType<any>>()

function getCachedLazy(loader: LazyLoader): ComponentType<any> {
  let Lazy = lazyCache.get(loader)
  if (!Lazy) {
    Lazy = lazy(loader)
    lazyCache.set(loader, Lazy)
  }
  return Lazy
}

export function LazyRouteRenderer({
  component,
  fallback = null,
}: {
  component: ComponentLike
  fallback?: ReactNode
}) {
  // A dynamic-import loader is an anonymous 0-arg function that is NOT a React
  // class component. React elements and normal components fall through.
  const isLikelyLazy =
    typeof component === 'function' &&
    (component as (...a: any[]) => any).length === 0 &&
    !(component as any).prototype?.isReactComponent &&
    (!(component as any).name || (component as any).name === '')

  const rendered = useMemo(() => {
    if (isValidElement(component)) return component
    if (isLikelyLazy) {
      const Lazy = getCachedLazy(component as LazyLoader)
      return <Lazy />
    }
    const Component = component as ComponentType<any>
    return <Component />
  }, [component, isLikelyLazy])

  if (isLikelyLazy) {
    return <Suspense fallback={fallback}>{rendered}</Suspense>
  }
  return <>{rendered}</>
}
