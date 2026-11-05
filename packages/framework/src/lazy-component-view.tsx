import { Suspense, lazy, useMemo } from 'react'
import type { ComponentType, ReactElement, ReactNode } from 'react'
import {
  classifyComponentLike,
  isThenable,
  warnLoaderRenderedAsComponent,
  warnUnrenderable,
  type LazyLoader,
} from './lazy-component'

// The ONE implementation behind every "render whatever a module put in this
// field" surface: `@ziee/framework/router`'s `LazyRouteRenderer` (routes) and
// `@ziee/shell`'s `LazyComponentRenderer` (slots, banners, shell components).
// They had independent copies of the same classification heuristic and so had
// the same bug twice; see `lazy-component.ts` for what the heuristic got wrong.

/** Anything a module may put in a `component` / `element` field. */
export type ComponentLike = ComponentType<any> | LazyLoader | ReactNode

/**
 * `lazy()` MUST be called once per loader for the app's lifetime — every call
 * returns a fresh lazy *type*, and React remounts the subtree when the type
 * changes. Keying by loader identity keeps the type stable across renders (a
 * fresh `<BrowserRouter>` mid-boot would otherwise strand `navigate()` on a
 * dead history).
 */
const lazyCache = new WeakMap<LazyLoader, ComponentType<any>>()

/**
 * A loader's module once it has resolved — the component a NEW render uses
 * directly. React 19's `lazy()` suspends on a type's FIRST render even when
 * the import has already resolved (its initializer attaches `.then` and
 * throws the thenable; the resolve lands a microtask later), so a route the
 * app had preloaded still painted its Suspense fallback for a frame — the
 * consumer's "the skeleton makes it look like flashing" (2026-11-05).
 */
const resolvedLoaders = new WeakMap<LazyLoader, ComponentType<any>>()

function recordResolved(loader: LazyLoader, mod: unknown): void {
  const C = (mod as { default?: ComponentType<any> } | null)?.default
  if (C) resolvedLoaders.set(loader, C)
}

function getCachedLazy(loader: LazyLoader): ComponentType<any> {
  let Lazy = lazyCache.get(loader)
  if (!Lazy) {
    Lazy = lazy(() =>
      loader().then(mod => {
        recordResolved(loader, mod)
        return mod
      }),
    )
    lazyCache.set(loader, Lazy)
  }
  return Lazy
}

/**
 * Load a loader's module and record it, so the next `RenderComponentLike` of
 * that loader renders the component directly — no first-render suspend. An
 * app's preload (a held navigation's route-element warm) calls this; a
 * rejected import records nothing (the render's own boundary answers it).
 */
export function primeLazyLoader(
  loader: LazyLoader,
  /** The import already in flight for `loader` (the caller's own call) —
   *  awaited instead of calling the loader a second time. */
  pending?: PromiseLike<unknown>,
): Promise<void> {
  return Promise.resolve()
    .then(() => pending ?? loader())
    .then(
      mod => recordResolved(loader, mod),
      () => undefined,
    )
}

/**
 * Stable default for `props`. A `props = {}` default literal is a fresh object
 * every render, which churns the `useMemo` deps below (and, before the lazy-type
 * cache, remounted the subtree).
 */
export const EMPTY_PROPS: Record<string, any> = Object.freeze({})

/**
 * A stable, distinct React key per ambiguous function.
 *
 * Before the tripwire existed, a named 0-arg function rendered as
 * `<Component {...props}/>` — its own element TYPE, so swapping the field to a
 * DIFFERENT such function changed the type and React remounted, discarding the
 * previous instance's hook state. Routing every one of them through the single
 * `AmbiguousComponent` type would silently reuse one fiber across that swap,
 * carrying hook state (and hook COUNT) from one component into another. Keying
 * the element by function identity restores the remount.
 */
const ambiguousKeys = new WeakMap<object, string>()
let ambiguousSequence = 0

function ambiguousKey(fn: object): string {
  let key = ambiguousKeys.get(fn)
  if (!key) {
    key = `ambiguous:${++ambiguousSequence}`
    ambiguousKeys.set(fn, key)
  }
  return key
}

/**
 * The runtime tripwire for the one case classification cannot settle: a NAMED
 * 0-arg function whose source shows no `import(`. Rendering it is the right
 * default (it is far more often a component), but if it returns a Promise it
 * was a loader — and handing React an uncached promise means the enclosing
 * boundary never settles and the region is empty for the life of the page, with
 * nothing logged. Here it is caught, named, and rendered as nothing instead.
 */
function AmbiguousComponent({
  fn,
  props,
  debugId,
}: {
  fn: ComponentType<any>
  props: Record<string, any>
  debugId?: string
}) {
  const result = (fn as (p: Record<string, any>) => unknown)(props)
  if (isThenable(result)) {
    warnLoaderRenderedAsComponent(debugId, fn.name || '(anonymous)')
    return null
  }
  return result as ReactElement | null
}

export interface RenderComponentLikeProps {
  component: ComponentLike
  props?: Record<string, any>
  fallback?: ReactNode
  /**
   * Slot key, route path, or banner id. Purely diagnostic — it is what turns
   * "some region is empty" into a message naming the entry at fault.
   */
  debugId?: string
}

/** Materialize a component-like value, owning the `<Suspense>` boundary when
 *  (and only when) the value needs one. */
export function RenderComponentLike({
  component,
  props = EMPTY_PROPS,
  fallback = null,
  debugId,
}: RenderComponentLikeProps) {
  const { kind, reason, ambiguous } = useMemo(
    () => classifyComponentLike(component),
    [component],
  )

  // Decided ONCE per component identity: a loader already resolved renders
  // its component directly; one that is not stays on its lazy type for this
  // mount even after it resolves (switching the element type under a live
  // page would remount it). The Suspense wrapper below stays either way, so
  // the tree's shape never depends on the choice.
  const direct = useMemo(
    () => (kind === 'loader' ? (resolvedLoaders.get(component as LazyLoader) ?? null) : null),
    [kind, component],
  )

  const rendered = useMemo(() => {
    switch (kind) {
      case 'element':
        return component as ReactElement
      case 'loader': {
        if (direct) {
          const Direct = direct
          return <Direct {...props} />
        }
        const Lazy = getCachedLazy(component as LazyLoader)
        return <Lazy {...props} />
      }
      case 'react-lazy': {
        const Lazy = component as ComponentType<any>
        return <Lazy {...props} />
      }
      case 'component': {
        const Component = component as ComponentType<any>
        if (ambiguous) {
          return (
            <AmbiguousComponent
              key={ambiguousKey(Component)}
              fn={Component}
              props={props}
              debugId={debugId}
            />
          )
        }
        return <Component {...props} />
      }
      case 'invalid':
        warnUnrenderable(debugId, reason)
        return null
    }
  }, [component, props, kind, reason, ambiguous, debugId, direct])

  // Only the two lazy kinds can suspend by design; everything else renders
  // synchronously and must not be wrapped (a stray boundary swallows a real
  // suspension from further down).
  if (kind === 'loader' || kind === 'react-lazy') {
    return <Suspense fallback={fallback}>{rendered}</Suspense>
  }
  return <>{rendered}</>
}
