/**
 * Pure registry logic — NO `import.meta.glob` (so it unit-tests in plain Node,
 * without vite). The app's `support/registry.ts` layers the eager glob on top of
 * these; `import.meta.glob` is Vite-only and cannot cross a package boundary, so
 * discovery stays app-side and is injected (see `GalleryConfig.discoverGalleries`).
 */
import type { Cassette } from '../mock/mockApi'
import type { ModuleGallery } from './types'

/** One discovered per-module seed: its module name + the exported `gallery`. */
export interface DiscoveredGallery {
  module: string
  gallery: ModuleGallery
}

/** `../../../modules/foo/gallery.tsx` → `foo`. */
export function moduleNameFromPath(path: string): string {
  const m = path.match(/modules\/([^/]+)\/gallery\.(?:ts|tsx)$/)
  return m ? m[1] : path
}

/**
 * Merge per-module cassettes into one. Each module seeds only ITS OWN endpoints,
 * so a duplicate endpoint key across two modules is a real ambiguity → THROW
 * (dev-only, surfaces at gallery boot). The shared crawl base is applied
 * separately (and always overridden) in the app's fixtures barrel.
 */
export function mergeModuleCassettes(galleries: DiscoveredGallery[]): Cassette {
  const out: Cassette = {}
  const owner: Record<string, string> = {}
  for (const { module, gallery } of galleries) {
    if (!gallery.cassette) continue
    for (const key of Object.keys(gallery.cassette) as (keyof Cassette)[]) {
      if (owner[key as string]) {
        throw new Error(
          `[gallery] cassette collision on "${String(key)}": both "${owner[key as string]}" and "${module}" seed it. One module must own each endpoint.`,
        )
      }
      owner[key as string] = module
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      ;(out as any)[key] = (gallery.cassette as any)[key]
    }
  }
  return out
}

/** Throw on a duplicate slug across ALL surface classes. */
export function assertUniqueSlugs(galleries: DiscoveredGallery[]): void {
  const seen: Record<string, string> = {}
  const check = (slug: string, module: string, cls: string) => {
    const prev = seen[slug]
    if (prev) {
      throw new Error(
        `[gallery] duplicate surface slug "${slug}" — ${prev} and ${module}/${cls}. Slugs must be unique across overlays/deep/seeded.`,
      )
    }
    seen[slug] = `${module}/${cls}`
  }
  for (const { module, gallery } of galleries) {
    for (const o of gallery.overlays ?? []) check(o.slug, module, 'overlay')
    for (const d of gallery.deepStates ?? []) check(d.slug, module, 'deep')
    for (const s of gallery.seeded ?? []) check(s.slug, module, 'seeded')
  }
}
