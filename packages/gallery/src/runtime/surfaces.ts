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
 * Enumerate EVERY gallery surface across all four classes. Pages are read from
 * the rendered browse DOM (they only exist after the router store populates);
 * the other three are the assembled entry lists. Call this on the browse canvas
 * (no `?surface=`) so the page list is populated.
 */
export function listAllSurfaces(): GallerySurfaceClasses {
  const OVERLAY_SLUGS = overlaySlugs()
  const DEEP_SLUGS = deepSlugs()
  const SEEDED_SLUGS = seededSlugs()
  const special = new Set([...OVERLAY_SLUGS, ...DEEP_SLUGS, ...SEEDED_SLUGS])
  const pages =
    typeof document !== 'undefined'
      ? Array.from(document.querySelectorAll('[data-testid^="gallery-page-"]'))
          .map(el =>
            (el.getAttribute('data-testid') || '').replace('gallery-page-', ''),
          )
          .filter(id => id && !special.has(id))
      : []
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
