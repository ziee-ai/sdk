/**
 * Lazy-render helpers for per-module `gallery.tsx` files — mount a real
 * component (by named export) with optional fixed props, or compose several
 * sections. Component CODE is always `import()`-split; only fixture DATA is eager.
 */
import { type ComponentType, type LazyExoticComponent, lazy } from 'react'

/** Lazy-load a named export as the surface component. */
export const lazyNamed = (loader: () => Promise<any>, name: string) =>
  lazy(() => loader().then(m => ({ default: m[name] })))

/** Lazy-load a named export and render it with fixed props (prop-taking components). */
export const lazyProps = (
  loader: () => Promise<any>,
  name: string,
  props: Record<string, unknown>,
): LazyExoticComponent<ComponentType> =>
  lazy(async () => {
    const C = (await loader())[name] as ComponentType<any>
    return { default: () => <C {...props} /> }
  })

/** Prop-driven overlays whose visibility is a parent-passed `open` prop (not a
 *  store). The overlay analog of `lazyNamed`. Props are cast (dev-only fixtures). */
export const lazyBound = (
  loader: () => Promise<any>,
  name: string,
  props: Record<string, unknown>,
): LazyExoticComponent<ComponentType> =>
  lazy(async () => {
    const C = (await loader())[name] as ComponentType<any>
    return { default: () => <C {...(props as any)} /> }
  })

/** Compose several named exports into one rendered column (multi-section pages). */
export const lazyCompose = (
  parts: { loader: () => Promise<any>; name: string }[],
): LazyExoticComponent<ComponentType> =>
  lazy(async () => {
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
