/**
 * `GalleryConfig` — the dependency-injection surface the app hands `mountGallery`.
 *
 * The gallery FRAMEWORK is app-agnostic: everything it used to reach through the
 * app's `@/` alias (the api-client route table, the module loader / router store /
 * auth seed, the app `ThemeProvider` / error boundary / loading + lazy renderers,
 * the accent tokens + theme writers) is injected here. `@ziee/kit` +
 * `@ziee/framework/stores` stay hard package deps. Surface DISCOVERY stays app-side
 * (`import.meta.glob` is Vite-only and cannot cross the package boundary) and is
 * injected via `discoverGalleries`.
 */
import type { ComponentType, LazyExoticComponent, ReactNode } from 'react'
import type { Cassette, SpecialRoute } from '../mock/mockApi'
import type { DiscoveredGallery } from '../registry/registry-core'
import type { GalleryStory } from './story'

/** Auth/role seed for permission-state coverage. */
export type AuthSeed = 'admin' | 'limited' | 'none'

/** Minimal route shape the page enumerator reads from the app's router store. */
export interface RouteLike {
  path?: string
  element?: unknown
}

/** The app's router-store hook (selector form) — `useRoutesStore(s => s.routes)`.
 *  Loosely typed (a DI seam): the app's concrete zustand hook assigns cleanly. */
/** The app's routes store hook.
 *
 *  `getState` is declared because enumeration (`listAllSurfaces`) must read the
 *  route list OUTSIDE React — it runs from a Playwright `page.evaluate`, not a
 *  render. Every zustand store hook carries it; it is optional here so an app
 *  passing a hand-rolled hook still type-checks (enumeration then falls back to
 *  the DOM scrape). */
export type RoutesStoreHook = ((selector: (state: any) => any) => any) & {
  getState?: () => any
}

/** The app's error boundary — rendered with `label` + a render-prop `fallback`.
 *  Loosely typed so the app's concrete boundary component assigns cleanly. */
export type ErrorBoundaryComponent = ComponentType<any>

/** Deep-state frame wiring (the single component every deep entry mounts). */
export interface DeepStateConfig {
  /** The component each deep entry renders (e.g. chat's ConversationPage). */
  component: LazyExoticComponent<ComponentType>
  /** Route path it mounts under (e.g. `/chat/:conversationId`). */
  routePath: string
  /** Concrete initial path for an entry's conversation id. */
  buildInitialPath: (conversationId: string) => string
}

export interface GalleryConfig {
  // ── surface discovery (app runs the Vite glob) ────────────────────────────
  /** App: `import.meta.glob('./modules/**\/gallery.{ts,tsx}', { eager: true })`
   *  → the discovered per-module galleries. */
  discoverGalleries: () => DiscoveredGallery[]

  // ── mock api ──────────────────────────────────────────────────────────────
  /** The app's generated `ApiEndpoints` map (`"NS.method"` → `"METHOD /url"`). */
  apiEndpoints: Record<string, string>
  /** Shared recorded crawl BASE cassette (broad param-less GETs). Module
   *  cassettes are layered OVER this, winning per key. */
  crawlCassette?: Cassette<any>
  /** App-specific endpoints answered outside the JSON table (SSE / binary / text). */
  specialRoutes?: SpecialRoute[]
  /** Override the error-mode exemption regexes (default: auth/me, setup, health). */
  errorModeExempt?: RegExp[]

  // ── app framework surfaces the harness renders through ────────────────────
  loadModules: () => void
  seedAuth: (auth: AuthSeed) => void
  useRoutesStore: RoutesStoreHook
  ThemeProvider: ComponentType<{ children: ReactNode }>
  ErrorBoundary: ErrorBoundaryComponent
  Loading: ComponentType<any>
  LazyComponentRenderer: ComponentType<any>

  // ── theme matrix (app tokens) ─────────────────────────────────────────────
  /** Every user-selectable accent (control-bar options + screenshot sweep). */
  accents: string[]
  /** Accent → human label for the control bar. */
  accentLabels: Record<string, string>
  /** Default accent when the URL omits `?accent=`. */
  defaultAccent: string
  /** Write the theme preference through the app's real config store. */
  setThemePref: (theme: 'light' | 'dark') => void
  /** Write the accent preference through the app's real config store. */
  setAccentPref: (accent: string) => void

  // ── content the harness composes ──────────────────────────────────────────
  /** Central kit-component stories (module stories are auto-collected). */
  stories?: GalleryStory[]
  /** Concrete values for required route params (`providerId`, …). */
  paramValues?: Record<string, string | undefined>
  /** Deep-state frame wiring (omit if the app has no deep states). */
  deepState?: DeepStateConfig
  /** Route paths that are not reviewable page CONTENT (redirects, callbacks). */
  skipPaths?: string[]
  /** Per-surface default auth seed (e.g. `{ auth: 'none' }` for the login form). */
  surfaceAuthSeed?: Record<string, AuthSeed>
  /** Baselined runtime findings passed through to the runtime-health pass. */
  runtimeBaseline?: unknown[]
  /** DOM id to mount into (default `root`). */
  rootElementId?: string
}

let active: GalleryConfig | undefined

/** Set the active gallery config (called once by `mountGallery`). */
export function setGalleryConfig(cfg: GalleryConfig): void {
  active = cfg
}

/** Read the active gallery config (throws if `mountGallery` hasn't run). */
export function getGalleryConfig(): GalleryConfig {
  if (!active) throw new Error('[gallery] config not set — call mountGallery() first')
  return active
}
