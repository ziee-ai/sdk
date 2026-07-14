// @ziee/gallery — the render-every-surface × states × themes visual-testing
// FRAMEWORK. The app supplies its own surfaces (per-module `gallery.tsx`),
// cassettes, and coverage maps; this package supplies the runner, the four gates
// (gate:ui + runtime-health + coverage + prod-exclusion, under `scripts/`), and
// the registry generators. Everything the framework used to reach through the
// app's `@/` alias is injected via `mountGallery(GalleryConfig)`.

// ── boot ──────────────────────────────────────────────────────────────────
export { mountGallery } from './runtime/mount'
export { GalleryPage } from './runtime/GalleryPage'
export {
  type GalleryConfig,
  type AuthSeed,
  type RouteLike,
  type RoutesStoreHook,
  type ErrorBoundaryComponent,
  type DeepStateConfig,
  getGalleryConfig,
  setGalleryConfig,
} from './runtime/config'

// ── authoring contract ──────────────────────────────────────────────────────
export type {
  ModuleGallery,
  OverlayEntry,
  DeepStateEntry,
  SeededSurfaceEntry,
} from './registry/types'
export type {
  Cassette,
  CassetteEntry,
  MockRequestContext,
  SseFrame,
  SpecialRoute,
  MockMode,
} from './mock/mockApi'
export type { GalleryStory, GalleryCase } from './runtime/story'
export type {
  InteractionRecipe,
  PageDriver,
  HasInteractions,
  InteractionManifestEntry,
} from './runtime/interactions'

// ── pure registry (unit-testable) ───────────────────────────────────────────
export {
  type DiscoveredGallery,
  mergeModuleCassettes,
  assertUniqueSlugs,
  moduleNameFromPath,
} from './registry/registry-core'

// ── mock engine ─────────────────────────────────────────────────────────────
export {
  configureMockApi,
  installMockApi,
  uninstallMockApi,
  setCassette,
  extendCassette,
  setMockMode,
  getMockMode,
  setSseCassette,
  sseResponse,
  sseReplayResponse,
  jsonResponse,
  mockErrorResponse,
} from './mock/mockApi'
export { makeBinaryResponse, base64ToBytes } from './mock/mockApi-binary'

// ── surfaces + assembled registry ───────────────────────────────────────────
export {
  type GallerySurfaceClasses,
  listAllSurfaces,
  overlaySlugs,
  deepSlugs,
  seededSlugs,
  interactionManifest,
} from './runtime/surfaces'
export {
  initSurfaces,
  getModuleCassette,
  getOverlayEntries,
  getDeepStateEntries,
  getSeededSurfaceEntries,
  getModuleStories,
  overlayBySlug,
  deepStateBySlug,
  seededSurfaceBySlug,
} from './runtime/surfaces-registry'

// ── matrix / theming ────────────────────────────────────────────────────────
export {
  type GalleryTheme,
  type GalleryDir,
  type GalleryViewport,
  type GalleryParams,
  GALLERY_THEMES,
  GALLERY_DIRS,
  GALLERY_VIEWPORTS,
  GALLERY_PATH,
  GALLERY_STANDALONE_PATH,
  parseGalleryParams,
} from './runtime/matrix'

// ── story / interaction / lazy / hold helpers ───────────────────────────────
export { StorySection, sectionTestId, caseTestId } from './runtime/story'
export {
  makeDomDriver,
  runInteraction,
  requestedInteraction,
  useRunInteraction,
  buildInteractionManifest,
} from './runtime/interactions'
export { lazyNamed, lazyProps, lazyBound, lazyCompose } from './runtime/lazy'
export { whenTrue, holdPatch, holdForever } from './runtime/hold'
export { GalleryPages, useResolvedPages, pageTestId } from './runtime/pages'
export { useGalleryTheme } from './runtime/useGalleryTheme'
