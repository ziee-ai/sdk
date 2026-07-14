/**
 * The assembled surface registry — populated once by `mountGallery` from the
 * app-injected `discoverGalleries()`. Discovery (`import.meta.glob`) stays
 * app-side; this module holds the MERGED result (the pure merge/assert lives in
 * `registry/registry-core.ts`) and the per-class `bySlug` lookups the frames read.
 *
 * The eager, synchronous assembly ordering is preserved: `mountGallery` calls
 * `initSurfaces(discovered)` BEFORE installing the mock API + rendering, so the
 * cassette is complete when the first store loads.
 */
import type { Cassette } from '../mock/mockApi'
import {
  type DiscoveredGallery,
  assertUniqueSlugs,
  mergeModuleCassettes,
} from '../registry/registry-core'
import type {
  DeepStateEntry,
  OverlayEntry,
  SeededSurfaceEntry,
} from '../registry/types'
import type { GalleryStory } from './story'

interface AssembledSurfaces {
  discovered: DiscoveredGallery[]
  cassette: Cassette
  overlays: OverlayEntry[]
  deepStates: DeepStateEntry[]
  seeded: SeededSurfaceEntry[]
  stories: GalleryStory[]
}

let assembled: AssembledSurfaces = {
  discovered: [],
  cassette: {},
  overlays: [],
  deepStates: [],
  seeded: [],
  stories: [],
}

/** Assemble the four surface classes + the merged cassette from discovery. */
export function initSurfaces(discovered: DiscoveredGallery[]): AssembledSurfaces {
  assertUniqueSlugs(discovered)
  assembled = {
    discovered,
    cassette: mergeModuleCassettes(discovered),
    overlays: discovered.flatMap(g => g.gallery.overlays ?? []),
    deepStates: discovered.flatMap(g => g.gallery.deepStates ?? []),
    seeded: discovered.flatMap(g => g.gallery.seeded ?? []),
    stories: discovered.flatMap(g => g.gallery.stories ?? []),
  }
  return assembled
}

export const getModuleCassette = (): Cassette => assembled.cassette
export const getOverlayEntries = (): OverlayEntry[] => assembled.overlays
export const getDeepStateEntries = (): DeepStateEntry[] => assembled.deepStates
export const getSeededSurfaceEntries = (): SeededSurfaceEntry[] => assembled.seeded
export const getModuleStories = (): GalleryStory[] => assembled.stories

export const overlayBySlug = (slug: string): OverlayEntry | undefined =>
  assembled.overlays.find(o => o.slug === slug)
export const deepStateBySlug = (slug: string): DeepStateEntry | undefined =>
  assembled.deepStates.find(e => e.slug === slug)
export const seededSurfaceBySlug = (slug: string): SeededSurfaceEntry | undefined =>
  assembled.seeded.find(e => e.slug === slug)
