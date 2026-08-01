/**
 * Lazy-render helpers for per-module `gallery.tsx` files — mount a real
 * component (by named export) with optional fixed props, or compose several
 * sections. Component CODE is always `import()`-split; only fixture DATA is eager.
 */
import { type ComponentType, type LazyExoticComponent, lazy } from 'react'

/**
 * A lazy surface component that ALSO exposes `preload()` — resolving its
 * `import()` up front (dedup'd) so the component can later render WITHOUT
 * suspending. The gallery frames call `preload()` and gate the surface behind a
 * `Loading` fallback until it resolves; this eliminates the mid-render Suspense
 * SUSPEND during capture. Under the concurrent runtime-health pass, a mid-render
 * suspend's reveal is timed by the shared vite dev server's HTTP response order
 * (network-decoupled from the page's own effects/StrictMode passes), and that
 * adversarial reveal interleaving is what trips React 19.2's dev-only
 * "Expected static flag was missing" invariant on the revealed forwardRef. With
 * the module pre-resolved, `<Component/>` renders synchronously (no suspend, no
 * network-timed reveal) — so the class of dev false-positive cannot arise, while
 * the pass keeps running fully concurrently. (In prod / normal app use the
 * component's code is likewise already loaded before it renders open, so this
 * matches real semantics.) */
export type PreloadableComponent = LazyExoticComponent<ComponentType> & {
  /** Warm the surface's `import()` up front (dedup'd), SHARING the underlying
   *  factory with `lazy()`. Once resolved, rendering `<Component/>` resolves the
   *  lazy from the module cache (a local microtask) instead of a network fetch,
   *  so the Suspense reveal is no longer timed by the shared vite dev server's
   *  concurrent HTTP-response order (the thing that made the reveal cross a
   *  StrictMode double-invoke and trip React 19.2's dev-only static-flag
   *  invariant under the concurrent pass). Returns the resolved module. */
  preload: () => Promise<{ default: ComponentType }>
}

/**
 * Build a preloadable surface component from a factory that resolves to
 * `{ default: Component }` (the React.lazy contract). The factory runs at most
 * once (cached) and is SHARED between `lazy(run)` and `preload()`, so warming
 * the cache with `preload()` makes the later `lazy` render resolve from cache. */
function makePreloadable(
  factory: () => Promise<{ default: ComponentType }>,
): PreloadableComponent {
  let cached: Promise<{ default: ComponentType }> | null = null
  const run = () => (cached ??= factory())
  const Comp = lazy(run) as PreloadableComponent
  Comp.preload = run
  return Comp
}

/** Lazy-load a named export as the surface component. */
export const lazyNamed = (loader: () => Promise<any>, name: string) =>
  makePreloadable(() => loader().then(m => ({ default: m[name] as ComponentType })))

/** Lazy-load a named export and render it with fixed props (prop-taking components). */
export const lazyProps = (
  loader: () => Promise<any>,
  name: string,
  props: Record<string, unknown>,
): PreloadableComponent =>
  makePreloadable(async () => {
    const C = (await loader())[name] as ComponentType<any>
    return { default: () => <C {...props} /> }
  })

/** Prop-driven overlays whose visibility is a parent-passed `open` prop (not a
 *  store). The overlay analog of `lazyNamed`. Props are cast (dev-only fixtures). */
export const lazyBound = (
  loader: () => Promise<any>,
  name: string,
  props: Record<string, unknown>,
): PreloadableComponent =>
  makePreloadable(async () => {
    const C = (await loader())[name] as ComponentType<any>
    return { default: () => <C {...(props as any)} /> }
  })

/** Compose several named exports into one rendered column (multi-section pages). */
export const lazyCompose = (
  parts: { loader: () => Promise<any>; name: string }[],
): PreloadableComponent =>
  makePreloadable(async () => {
    const mods = await Promise.all(parts.map(p => p.loader()))
    const Comps = mods.map((m, i) => m[parts[i].name] as ComponentType)
    return {
      default: () => (
        <div className="flex flex-col gap-4 p-4">
          {Comps.map((C, i) => (
            <C key={i} />
          ))}
        </div>
      ),
    }
  })
