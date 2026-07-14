/**
 * Page + surface frames for the seeded gallery — every REAL module route (and
 * overlay / deep / seeded surface) rendered inside an isolated `MemoryRouter`,
 * populated through the mock-API cassette.
 *
 * Pages are ENUMERATED AT RENDER TIME from the app's router store (populated by
 * `mountGallery` → `loadModules()`), so every route a module registers is covered
 * automatically. Each frame gives the surface a bounded, sized viewport and its
 * own router so `useParams`/`useNavigate` stay isolated per entry.
 *
 * Testid convention: each surface → `gallery-page-<id>`.
 */
import { type ReactNode, Suspense, useEffect } from 'react'
import { MemoryRouter, Route, Routes } from 'react-router-dom'
import { Text, Title } from '@ziee/kit'
import { type RouteLike, getGalleryConfig } from './config'
import {
  deepStateBySlug,
  overlayBySlug,
  seededSurfaceBySlug,
} from './surfaces-registry'
import type {
  DeepStateEntry,
  OverlayEntry,
  SeededSurfaceEntry,
} from '../registry/types'
import { useRunInteraction } from './interactions'

export const pageTestId = (id: string) => `gallery-page-${id}`

/**
 * Fallback for a per-surface error boundary. Renders a DETECTABLE marker
 * (`data-testid="gallery-crash"`) so the capture layer can assert on a REAL
 * boundary render in the settled DOM. `label` identifies which surface threw.
 */
function galleryCrashFallback(label: string) {
  return (error: Error) => (
    <div
      data-testid="gallery-crash"
      data-crash-label={label}
      className="flex h-full w-full items-center justify-center p-4 text-center text-sm text-destructive"
    >
      Surface crashed: {error.message}
    </div>
  )
}

// Detail-route params come from the injected `paramValues` OR the URL (so an
// isolated combo can pin a specific `conversationId` / `projectId`). URL wins.
function urlParams(): Record<string, string> {
  const q = new URLSearchParams(window.location.search)
  const out: Record<string, string> = {}
  for (const [k, v] of q) out[k] = v
  return out
}

function paramValues(): Record<string, string | undefined> {
  return { ...getGalleryConfig().paramValues, ...urlParams() }
}

interface ResolvedPage {
  id: string
  path: string
  initialPath: string
  element: RouteLike['element']
}

/**
 * path → stable slug for the testid (`/settings/llm-providers/:x?` →
 * `settings-llm-providers`). Routes with a REQUIRED param get a `-detail` suffix
 * so a swap-type detail route doesn't collide with its list route.
 */
function slugFor(path: string): string {
  const requiredParam = path
    .split('/')
    .some(s => s.startsWith(':') && !s.endsWith('?'))
  const cleaned = path
    .replace(/\/:[^/?]+\??/g, '') // drop param segments
    .replace(/^\/+|\/+$/g, '')
    .replace(/\//g, '-')
  const base = cleaned || 'root'
  return requiredParam ? `${base}-detail` : base
}

/** Fill a route path's params; return undefined if a REQUIRED param is unresolved. */
function resolveInitialPath(path: string): string | undefined {
  const values = paramValues()
  const segments = path.split('/')
  const out: string[] = []
  for (const seg of segments) {
    if (!seg.startsWith(':')) {
      out.push(seg)
      continue
    }
    const optional = seg.endsWith('?')
    const name = seg.slice(1, optional ? -1 : undefined)
    const value = values[name]
    if (value) out.push(value)
    else if (optional) continue // drop the optional segment
    else return undefined // required + unresolved → skip page
  }
  return out.join('/') || '/'
}

/** Build the ordered, de-duplicated page list from the router store. */
export function useResolvedPages(): ResolvedPage[] {
  const cfg = getGalleryConfig()
  const routes = cfg.useRoutesStore(s => s.routes) as RouteLike[]
  const skip = new Set(cfg.skipPaths ?? ['/', '/dev/gallery', '/auth/callback'])
  const seen = new Set<string>()
  const pages: ResolvedPage[] = []
  for (const route of routes) {
    if (!route.path || skip.has(route.path)) continue
    const initialPath = resolveInitialPath(route.path)
    if (initialPath === undefined) continue
    const id = slugFor(route.path)
    if (seen.has(id)) continue
    seen.add(id)
    if (!route.element) continue
    pages.push({ id, path: route.path, initialPath, element: route.element })
  }
  // Stable, reviewable order.
  return pages.sort((a, b) => a.id.localeCompare(b.id))
}

function PageFrame({
  page,
  state = 'loaded',
  height = 720,
}: {
  page: ResolvedPage
  state?: string
  height?: number
}): ReactNode {
  const { ErrorBoundary, Loading, LazyComponentRenderer } = getGalleryConfig()
  return (
    <section
      data-testid={pageTestId(page.id)}
      data-gallery-state={state}
      className="flex flex-col gap-3 border border-border rounded-lg p-4 bg-background"
    >
      <div className="flex flex-col gap-1" data-gallery-chrome>
        <Title level={3}>
          {page.path}
          {state !== 'loaded' ? (
            <Text tone="muted" className="ml-2 text-sm">
              · {state}
            </Text>
          ) : null}
        </Title>
        <Text tone="muted" className="text-sm">
          gallery-page-{page.id} · seeded via mock-API
        </Text>
      </div>
      <div
        data-gallery-frame
        className="w-full overflow-hidden rounded-md border border-border bg-background"
        style={{ height }}
      >
        <ErrorBoundary
          label={`page-${page.id}`}
          fallback={galleryCrashFallback(`page-${page.id}`)}
        >
          <MemoryRouter initialEntries={[page.initialPath]}>
            <Routes>
              <Route
                path={page.path}
                element={
                  <LazyComponentRenderer
                    component={page.element}
                    fallback={<Loading />}
                  />
                }
              />
            </Routes>
          </MemoryRouter>
        </ErrorBoundary>
      </div>
    </section>
  )
}

/** Renders an overlay in its OPEN state: fires the store open action on mount. */
function OverlayFrame({ entry }: { entry: OverlayEntry }) {
  const { ErrorBoundary, Loading } = getGalleryConfig()
  useEffect(() => {
    entry.open?.()
  }, [entry])
  // Overlays portal on mount; give the recipe a moment for the portal to paint.
  useRunInteraction(entry.interactions, 700)
  const Component = entry.component
  return (
    <section
      data-testid={pageTestId(entry.slug)}
      data-gallery-state="open"
      className="flex flex-col gap-3 border border-border rounded-lg p-4 bg-background"
    >
      <div className="flex flex-col gap-1" data-gallery-chrome>
        <Title level={3}>
          {entry.title}
          <Text tone="muted" className="ml-2 text-sm">
            · open
          </Text>
        </Title>
        <Text tone="muted" className="text-sm">
          gallery-page-{entry.slug} · overlay open-state
        </Text>
      </div>
      <ErrorBoundary
        label={`overlay-${entry.slug}`}
        fallback={galleryCrashFallback(`overlay-${entry.slug}`)}
      >
        <Suspense fallback={<Loading />}>
          <Component />
        </Suspense>
      </ErrorBoundary>
    </section>
  )
}

/** Renders one deep-state entry: the injected deep component + a mount-time seed. */
function DeepStateFrame({ entry }: { entry: DeepStateEntry }): ReactNode {
  const { ErrorBoundary, Loading, deepState } = getGalleryConfig()
  useEffect(() => {
    void entry.setup?.()
  }, [entry])
  // Deep surfaces need their seed + the lazy component to settle before an
  // interaction can find its target, so give the recipe a longer settle window.
  useRunInteraction(entry.interactions, 1200)
  if (!deepState) {
    throw new Error(
      '[gallery] a deepStates entry was requested but GalleryConfig.deepState is not set',
    )
  }
  const Component = deepState.component
  return (
    <section
      data-testid={pageTestId(entry.slug)}
      data-gallery-state="deep"
      className="flex flex-col gap-3 border border-border rounded-lg p-4 bg-background"
    >
      <div className="flex flex-col gap-1" data-gallery-chrome>
        <Title level={3}>
          {entry.title}
          <Text tone="muted" className="ml-2 text-sm">
            · deep-state
          </Text>
        </Title>
        <Text tone="muted" className="text-sm">
          gallery-page-{entry.slug} · {entry.note}
        </Text>
      </div>
      <div
        data-gallery-frame
        className="w-full overflow-hidden rounded-md border border-border bg-background"
        style={{ height: 720 }}
      >
        <ErrorBoundary label={`deep-${entry.slug}`} fallback={() => null}>
          <MemoryRouter initialEntries={[deepState.buildInitialPath(entry.conversationId)]}>
            <Routes>
              <Route
                path={deepState.routePath}
                element={
                  <Suspense fallback={<Loading />}>
                    <Component />
                  </Suspense>
                }
              />
            </Routes>
          </MemoryRouter>
        </ErrorBoundary>
      </div>
    </section>
  )
}

/** Renders one seeded-surface entry: the real component + a mount-time store seed. */
function SeededSurfaceFrame({ entry }: { entry: SeededSurfaceEntry }): ReactNode {
  const { ErrorBoundary, Loading } = getGalleryConfig()
  useEffect(() => {
    void entry.setup?.()
  }, [entry])
  useRunInteraction(entry.interactions, 1200)
  const Component = entry.component
  return (
    <section
      data-testid={pageTestId(entry.slug)}
      data-gallery-state="seeded"
      className="flex flex-col gap-3 border border-border rounded-lg p-4 bg-background"
    >
      <div className="flex flex-col gap-1" data-gallery-chrome>
        <Title level={3}>
          {entry.title}
          <Text tone="muted" className="ml-2 text-sm">
            · seeded
          </Text>
        </Title>
        <Text tone="muted" className="text-sm">
          gallery-page-{entry.slug} · {entry.note}
        </Text>
      </div>
      <div
        data-gallery-frame
        className={
          entry.fullHeight
            ? 'w-full rounded-md border border-border bg-background'
            : 'w-full overflow-hidden rounded-md border border-border bg-background'
        }
        style={entry.fullHeight ? undefined : { height: 720 }}
      >
        <ErrorBoundary label={`seeded-${entry.slug}`} fallback={() => null}>
          <MemoryRouter initialEntries={[entry.initialPath]}>
            <Routes>
              <Route
                path={entry.path}
                element={
                  <Suspense fallback={<Loading />}>
                    <Component />
                  </Suspense>
                }
              />
            </Routes>
          </MemoryRouter>
        </ErrorBoundary>
      </div>
    </section>
  )
}

/**
 * Render pages. With no `only`, browses every enumerated page (loaded). With
 * `only=<slug>`, renders just that surface in the given `state` — a page in a
 * data-state, or an overlay / deep / seeded surface in its open state.
 */
export function GalleryPages({ only, state }: { only?: string; state?: string }) {
  const pages = useResolvedPages()
  const deep = only ? deepStateBySlug(only) : undefined
  if (deep) return <DeepStateFrame entry={deep} />
  const seeded = only ? seededSurfaceBySlug(only) : undefined
  if (seeded) return <SeededSurfaceFrame entry={seeded} />
  const overlay = only ? overlayBySlug(only) : undefined
  if (overlay) return <OverlayFrame entry={overlay} />
  const shown = only ? pages.filter(p => p.id === only) : pages
  return (
    <>
      {shown.map(page => (
        <PageFrame key={page.id} page={page} state={state} />
      ))}
    </>
  )
}
