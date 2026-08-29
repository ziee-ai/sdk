/**
 * SINGLE SOURCE of gallery-surface enumeration.
 *
 * The gallery has FOUR surface classes rendered through different channels:
 *   - **pages**   — real module routes on the browse canvas (read from the DOM:
 *                   `[data-testid^="gallery-page-"]`);
 *   - **overlays**— interaction-only Sheet/Dialog open-states;
 *   - **deep**    — active-conversation deep-states;
 *   - **seeded**  — real components with a mount-time store seed.
 *
 * Everything (captures + coverage) enumerates through `listAllSurfaces()`
 * (published on `window.__GALLERY_LIST_ALL_SURFACES__`), so a new surface class
 * can never be missed by one tool but not another. The interaction-only classes
 * come from the assembled surface registry (populated by `mountGallery`).
 */
import {
  type InteractionManifestEntry,
  buildInteractionManifest,
} from './interactions'
import {
  getDeepStateEntries,
  getOverlayEntries,
  getSeededSurfaceEntries,
} from './surfaces-registry'
import { getGalleryConfig } from './config'
import { resolvePages } from './pages'

export interface GallerySurfaceClasses {
  pages: string[]
  overlays: string[]
  deep: string[]
  seeded: string[]
  interactions: InteractionManifestEntry[]
}

export const overlaySlugs = (): string[] => getOverlayEntries().map(o => o.slug)
export const deepSlugs = (): string[] => getDeepStateEntries().map(e => e.slug)
export const seededSlugs = (): string[] => getSeededSurfaceEntries().map(e => e.slug)

/** Flat interaction manifest across every interaction-bearing entry class. */
export const interactionManifest = (): InteractionManifestEntry[] =>
  buildInteractionManifest([
    ...getOverlayEntries(),
    ...getDeepStateEntries(),
    ...getSeededSurfaceEntries(),
  ])

/**
 * Enumerate EVERY gallery surface across all four classes.
 *
 * Pages come from the ROUTES STORE via `resolvePages`, not from the rendered
 * DOM. They used to be scraped out of `[data-testid^="gallery-page-"]`, which
 * made the surface list a function of whether the canvas finished rendering:
 * anything that threw at the top of the canvas silently SHORTENED the list every
 * capture / coverage / runtime-health run was built from, and the resulting
 * per-surface PASS table looked exactly like a clean one. Measured in a
 * consuming app — one story case with a `<Link>` outside a Router — the gate
 * enumerated 73 of 126 surfaces and reported `69/73 PASS`.
 *
 * The DOM scrape survives only as a FALLBACK, for a caller whose gallery config
 * has no routes store wired.
 */
export function listAllSurfaces(): GallerySurfaceClasses {
  const OVERLAY_SLUGS = overlaySlugs()
  const DEEP_SLUGS = deepSlugs()
  const SEEDED_SLUGS = seededSlugs()
  const special = new Set([...OVERLAY_SLUGS, ...DEEP_SLUGS, ...SEEDED_SLUGS])
  let pages: string[] = []
  try {
    const cfg = getGalleryConfig()
    const routes = cfg.useRoutesStore.getState?.().routes as Parameters<typeof resolvePages>[0]
    pages = resolvePages(routes ?? [], cfg.skipPaths)
      .map(p => p.id)
      .filter(id => id && !special.has(id))
  } catch {
    /* no config / no routes store — fall through to the DOM scrape */
  }
  if (pages.length === 0 && typeof document !== 'undefined') {
    pages = Array.from(document.querySelectorAll('[data-testid^="gallery-page-"]'))
      .map(el => (el.getAttribute('data-testid') || '').replace('gallery-page-', ''))
      .filter(id => id && !special.has(id))
  }
  return {
    // De-dup: a slug could appear twice if the browse canvas and a mounted frame
    // coexist in the DOM.
    pages: [...new Set(pages)],
    overlays: OVERLAY_SLUGS,
    deep: DEEP_SLUGS,
    seeded: SEEDED_SLUGS,
    interactions: interactionManifest(),
  }
}
