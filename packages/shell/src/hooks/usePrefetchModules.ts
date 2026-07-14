import { useEffect, isValidElement } from 'react'
import { Stores } from '@ziee/framework/stores'

/** The minimal route shape this hook reads off the app-registered `Routes`
 *  store (see the SEAM note below). */
interface PrefetchRoute {
  element: unknown
}

/**
 * Hook to prefetch lazy-loaded modules after initial render. Uses
 * `requestIdleCallback` to prefetch when the browser is idle.
 *
 * SEAM: reads the app-registered `Routes` store (populated by
 * `@ziee/framework/router` or an app-local router module) through a typed view,
 * so the shell doesn't depend on the router being present at type-check time.
 */
export function usePrefetchModules() {
  const { routes } = (
    Stores as unknown as { Routes: { routes: PrefetchRoute[] } }
  ).Routes

  useEffect(() => {
    // Check if requestIdleCallback is supported (not available in Safari < 16)
    const prefetch = () => {
      routes.forEach(route => {
        // If element is a function (preload function), call it to trigger the import
        if (
          typeof route.element === 'function' &&
          !isValidElement(route.element)
        ) {
          ;(
            route.element as () => Promise<{
              default: React.ComponentType<any>
            }>
          )()
        }
      })
    }

    if ('requestIdleCallback' in window) {
      const handle = requestIdleCallback(prefetch, { timeout: 2000 })
      return () => cancelIdleCallback(handle)
    } else {
      // Fallback for browsers without requestIdleCallback
      const timer = setTimeout(prefetch, 1000)
      return () => clearTimeout(timer)
    }
  }, [routes])
}
