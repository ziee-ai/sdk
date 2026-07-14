/**
 * `mountGallery(cfg)` — the boot API the app's thin `src/dev/gallery/main.tsx`
 * calls. It folds the former `main.tsx` + `seed.ts` into one framework entry:
 * assemble the injected surfaces, configure + install the mock API, seed auth,
 * load the app's modules, and render the gallery under the app's ThemeProvider.
 *
 * Ordering (unchanged from the pre-extraction seed): surfaces are assembled
 * EAGERLY (synchronously) and the mock API is installed BEFORE any store can fire
 * a load, so the cassette is complete when the first store reads.
 */
import React from 'react'
import ReactDOM from 'react-dom/client'
import {
  type AuthSeed,
  type GalleryConfig,
  getGalleryConfig,
  setGalleryConfig,
} from './config'
import { GalleryPage } from './GalleryPage'
import {
  type MockMode,
  configureMockApi,
  installMockApi,
  setMockMode,
} from '../mock/mockApi'
import { initSurfaces, getModuleCassette } from './surfaces-registry'
import {
  deepSlugs,
  interactionManifest,
  listAllSurfaces,
  overlaySlugs,
  seededSlugs,
} from './surfaces'

/** Publish the runtime manifest globals every capture/coverage/runtime pass reads. */
function publishSurfaceGlobals(): void {
  const w = window as unknown as Record<string, unknown>
  w.__GALLERY_OVERLAYS__ = overlaySlugs()
  w.__GALLERY_DEEP_STATES__ = deepSlugs()
  w.__GALLERY_SEEDED__ = seededSlugs()
  w.__GALLERY_INTERACTIONS__ = interactionManifest()
  // The SINGLE source every tool enumerates through (a function — pages are read
  // from the live DOM at call time).
  w.__GALLERY_LIST_ALL_SURFACES__ = listAllSurfaces
}

let seeded = false

function seedGallery(auth: AuthSeed): void {
  if (seeded) return
  seeded = true
  const cfg = getGalleryConfig()
  // 1. Mock backend (crawl base + module cassettes, module winning per key).
  installMockApi({ ...cfg.crawlCassette, ...getModuleCassette() })
  // 2. Auth/role seed (is_admin short-circuits every permission gate).
  cfg.seedAuth(auth)
  // 3. Register every module's stores + routes (lazy store init).
  cfg.loadModules()
}

export function mountGallery(cfg: GalleryConfig): void {
  setGalleryConfig(cfg)

  // Assemble the injected surfaces (eager, synchronous) + configure the engine.
  initSurfaces(cfg.discoverGalleries())
  configureMockApi({
    apiEndpoints: cfg.apiEndpoints,
    specialRoutes: cfg.specialRoutes,
    errorModeExempt: cfg.errorModeExempt,
  })
  publishSurfaceGlobals()

  // URL-driven multi-state rendering:
  //   ?surface=<slug>&state=<loaded|empty|error|delayed>&auth=<admin|limited|none>
  const q = new URLSearchParams(window.location.search)
  const surface = q.get('surface') ?? undefined
  const state = (q.get('state') as MockMode | null) ?? 'loaded'
  // Auth precedence: explicit `?auth=` → per-surface default → admin.
  const auth =
    (q.get('auth') as AuthSeed | null) ??
    (surface ? cfg.surfaceAuthSeed?.[surface] : undefined) ??
    'admin'

  // Set the data-state mode BEFORE any store loads (loads are lazy on first read).
  setMockMode(state)
  seedGallery(auth)

  const { ThemeProvider, ErrorBoundary } = cfg
  const rootEl = document.getElementById(cfg.rootElementId ?? 'root')
  if (!rootEl) throw new Error('[gallery] root element not found')
  ReactDOM.createRoot(rootEl).render(
    <React.StrictMode>
      <ErrorBoundary label="gallery" fallback={() => null}>
        <ThemeProvider>
          <GalleryPage surface={surface} state={state} />
        </ThemeProvider>
      </ErrorBoundary>
    </React.StrictMode>,
  )
}
