/**
 * Mock-API engine for the seeded gallery (generic over the app's api-client).
 *
 * Installs a `window.fetch` interceptor that answers `/api/*` requests from
 * per-module SEED CASSETTES instead of a live backend. Every store's REAL
 * `load()` path runs unchanged (same api-client → same fetch), so pages and
 * components populate exactly as in production — just deterministically and
 * offline. Dev-only; wired by `mountGallery()`.
 *
 * Correctness:
 *  1. cassette values are typed against the generated api-client response types
 *     (`Cassette<ApiEndpointResponses>` — a wrong shape fails `tsc`);
 *  2. cassettes are RECORDED from a real server;
 *  3. a contract test validates each cassette against `openapi.json`.
 *
 * Extraction seam: the engine is generic over the app's `ApiEndpoints` route
 * table (passed to `configureMockApi`) and over app-specific SSE / binary /
 * text endpoints (passed as `specialRoutes`). It hard-imports NO app module.
 */

/** Context handed to a cassette resolver function for one request. */
export interface MockRequestContext {
  /** Path captures, e.g. `{ provider_id: '…' }`. */
  params: Record<string, string>
  /** Parsed query string, e.g. `{ providerId: '…', page: '1' }`. */
  query: Record<string, string>
  /** Parsed JSON request body (mutations); `undefined` for GET/empty. */
  body: unknown
  method: string
}

/**
 * One cassette entry: either a literal recorded response, or a resolver that
 * derives it from the request (e.g. `LlmModel.list` keyed by `?providerId=`).
 */
export type CassetteEntry<TResp> =
  | TResp
  | ((ctx: MockRequestContext) => TResp)

/**
 * A partial map from endpoint key → recorded/derived response, GENERIC over the
 * app's generated `ApiEndpointResponses` map (`{ "NS.method": ResponseType }`).
 * The app binds `type Cassette = GCassette<ApiEndpointResponses>` so a shape
 * mismatch in any `gallery.tsx` fails the build. The default parameter keeps the
 * runtime functions callable with a type-erased cassette.
 */
export type Cassette<TResponses = Record<string, unknown>> = {
  [K in keyof TResponses]?: CassetteEntry<TResponses[K]>
}

/** A type-erased cassette — the shape the runtime functions accept. */
type AnyCassette = Cassette<any>

/**
 * An app-specific endpoint answered outside the JSON cassette table (an SSE
 * event-stream, raw binary bytes, extracted text). The app registers these via
 * `configureMockApi({ specialRoutes })`; the first matching one handles the
 * request BEFORE the data-state mode logic.
 */
export interface SpecialRoute {
  test: (path: string, method: string) => boolean
  respond: (ctx: {
    path: string
    method: string
    mode: MockMode
  }) => Response | Promise<Response>
}

interface CompiledRoute {
  key: string
  method: string
  regex: RegExp
  paramNames: string[]
}

let COMPILED: CompiledRoute[] = []
let specialRoutes: SpecialRoute[] = []
// Endpoints that must keep working even in `error` mode so the page can mount as
// an authenticated admin and render its OWN error state (not a login redirect).
let errorModeExempt: RegExp[] = [/\/auth\/me$/, /\/setup\/status$/, /\/health$/]

/**
 * Configure the engine's route table (from the app's generated `ApiEndpoints`)
 * plus any app-specific special routes / error-mode exemptions. Call ONCE before
 * `installMockApi`. Precompiles every endpoint URL pattern (`METHOD /api/x/{cap}`)
 * into a matcher so a concrete request path resolves back to its endpoint key.
 */
export function configureMockApi(opts: {
  apiEndpoints: Record<string, string>
  specialRoutes?: SpecialRoute[]
  errorModeExempt?: RegExp[]
}): void {
  COMPILED = Object.entries(opts.apiEndpoints).map(([key, url]) => {
    const [method, pattern] = (url as string).split(' ') as [string, string]
    const paramNames: string[] = []
    const source = pattern
      .replace(/[.*+?^${}()|[\]\\]/g, m => `\\${m}`) // escape regex metachars
      .replace(/\\\{([^}]+)\\\}/g, (_all, name: string) => {
        paramNames.push(name)
        return '([^/]+)'
      })
    return {
      key,
      method,
      regex: new RegExp(`^${source}$`),
      paramNames,
    }
  })
  specialRoutes = opts.specialRoutes ?? []
  if (opts.errorModeExempt) errorModeExempt = opts.errorModeExempt
}

function matchRoute(
  method: string,
  path: string,
): { route: CompiledRoute; params: Record<string, string> } | undefined {
  // Prefer the most specific match: a literal segment count tie-breaker keeps
  // `/api/llm-providers/{id}` from shadowing `/api/llm-providers`.
  let best: { route: CompiledRoute; params: Record<string, string> } | undefined
  for (const route of COMPILED) {
    if (route.method !== method) continue
    const m = route.regex.exec(path)
    if (!m) continue
    const params: Record<string, string> = {}
    route.paramNames.forEach((name, i) => {
      params[name] = decodeURIComponent(m[i + 1])
    })
    // Fewer captures = more literal = more specific → keep the tightest.
    if (!best || route.paramNames.length < best.route.paramNames.length) {
      best = { route, params }
    }
  }
  return best
}

let activeCassette: AnyCassette = {}
let installed = false
let originalFetch: typeof globalThis.fetch | undefined

// ── SSE replay (serverless token stream) ─────────────────────────────────────
// Live streaming states (streaming tokens, tool-call progress, elicitation
// prompts) arrive over an SSE connection, not a JSON endpoint. To exercise those
// states offline we REPLAY a recorded frame sequence: an SSE cassette is an
// ordered list of `{ event, data }` frames the interceptor serializes into a
// real `text/event-stream` body. The stream stays open after the last frame
// (like a real idle connection) so a mid-generation cassette leaves the UI
// visibly streaming. The app registers the stream PATH as a `specialRoute` that
// returns `sseReplayResponse()`.

/** One recorded SSE frame: the `event:` name + the JSON `data:` payload. */
export interface SseFrame {
  event: string
  data: unknown
}
let sseCassette: SseFrame[] = []
/** Register the frame sequence the next stream request replays. */
export function setSseCassette(frames: SseFrame[]): void {
  sseCassette = frames
}
const SSE_FRAME_GAP_MS = 350

/** Build an event-stream `Response` that replays the given frames. */
export function sseResponse(frames: SseFrame[]): Response {
  const encoder = new globalThis.TextEncoder()
  const stream = new ReadableStream<Uint8Array>({
    async start(controller) {
      // A `connected` handshake first (stream clients expect it to learn their
      // connection id), then each recorded frame with a small gap so the UI
      // paints the deltas progressively.
      controller.enqueue(
        encoder.encode(
          `event: connected\ndata: ${JSON.stringify({ connectionId: 'gallery-sse' })}\n\n`,
        ),
      )
      for (const f of frames) {
        await new Promise(r => setTimeout(r, SSE_FRAME_GAP_MS))
        controller.enqueue(
          encoder.encode(`event: ${f.event}\ndata: ${JSON.stringify(f.data)}\n\n`),
        )
      }
      // Leave the stream OPEN (never close) — a real idle SSE connection. The
      // gallery frame unmounts / navigates away to tear it down.
    },
  })
  return new Response(stream, {
    status: 200,
    headers: {
      'Content-Type': 'text/event-stream',
      'Cache-Control': 'no-cache',
      Connection: 'keep-alive',
    },
  })
}

/** Replay the currently-registered SSE cassette as an event-stream response. */
export const sseReplayResponse = (): Response => sseResponse(sseCassette)

/**
 * Data-state mode. The gallery renders the SAME surface under different modes to
 * cover the states where most bugs hide (empty / error):
 *   - loaded : the recorded response, unchanged;
 *   - empty  : every array in the response deep-emptied + counts zeroed;
 *   - error  : a 500 for data endpoints (auth/bootstrap exempt so the page still
 *              mounts authenticated and shows its error UI);
 *   - delayed: the loaded response after a latency, to catch the loading state.
 */
export type MockMode = 'loaded' | 'empty' | 'error' | 'delayed'
let activeMode: MockMode = 'loaded'
export function setMockMode(mode: MockMode): void {
  activeMode = mode
}
export function getMockMode(): MockMode {
  return activeMode
}

const DELAY_MS = 700

/** Deep-empty arrays + zero obvious counts, preserving object shape. */
function toEmpty(value: unknown): unknown {
  if (Array.isArray(value)) return []
  if (value && typeof value === 'object') {
    const out: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value)) {
      if (Array.isArray(v)) out[k] = []
      else if (typeof v === 'number' && /(total|count|pages|unread)/i.test(k)) out[k] = 0
      else out[k] = toEmpty(v)
    }
    return out
  }
  return value
}

/** Build a JSON `Response` (default 200). */
export const jsonResponse = (data: unknown, status = 200): Response =>
  new Response(JSON.stringify(data ?? null), {
    status,
    headers: { 'Content-Type': 'application/json' },
  })

/**
 * A realistic backend-shaped error body (NOT a "Gallery error state" dev
 * placeholder). Well-behaved surfaces render a human ErrorState and only ever
 * expose this string behind a "Details" disclosure. The `error_code` keeps the
 * gallery tooling's error-mode detection working.
 */
export const mockErrorResponse = (status = 500): Response =>
  jsonResponse(
    { error: 'Internal server error', error_code: 'GALLERY_ERROR' },
    status,
  )

// Crash-safe default for an UNRECORDED endpoint. An unseeded store must not
// crash the page it feeds — but no single literal shape fits every consumer
// (some read `res.items.map`, others `res.map`, others `res.total`). So the
// default is a recursive, array-like proxy.
const ARRAY_METHODS = new Set([
  'map', 'filter', 'forEach', 'reduce', 'reduceRight', 'find', 'findIndex',
  'some', 'every', 'slice', 'concat', 'flat', 'flatMap', 'includes',
  'indexOf', 'join', 'keys', 'values', 'entries', 'at', 'sort', 'reverse',
])

function makeSafeEmpty(): any {
  const target: any = []
  return new Proxy(target, {
    get(t, prop) {
      if (prop === Symbol.iterator) return [][Symbol.iterator].bind([])
      if (prop === 'length') return 0
      if (prop === 'toJSON') return () => []
      if (prop === Symbol.toPrimitive) return () => ''
      if (typeof prop === 'symbol') return (t as any)[prop]
      if (prop === 'then') return undefined // never a thenable
      if (ARRAY_METHODS.has(prop as string)) {
        return (...args: any[]) => (t as any)[prop](...args)
      }
      // Unknown property → recurse so deep access stays safe.
      return makeSafeEmpty()
    },
  })
}

/** Register (replace) the cassette the interceptor answers from. */
export function setCassette(cassette: AnyCassette): void {
  activeCassette = cassette
}

/** Merge additional entries into the active cassette. */
export function extendCassette(cassette: AnyCassette): void {
  activeCassette = { ...activeCassette, ...cassette }
}

/**
 * Install the `window.fetch` interceptor (idempotent). Non-`/api` requests and
 * app-registered special routes are handled first; everything else passes
 * through to the real fetch (vite assets, HMR).
 */
export function installMockApi(cassette?: AnyCassette): void {
  if (cassette) setCassette(cassette)
  if (installed) return
  installed = true
  originalFetch = globalThis.fetch.bind(globalThis)

  globalThis.fetch = async (
    input: RequestInfo | URL,
    init?: RequestInit,
  ): Promise<Response> => {
    const url =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.href
          : input.url
    const method = (
      init?.method ??
      (input instanceof Request ? input.method : 'GET')
    ).toUpperCase()

    let parsed: URL
    try {
      parsed = new URL(url, window.location.origin)
    } catch {
      return originalFetch!(input as RequestInfo, init)
    }

    // Only intercept same-origin API calls; everything else is real.
    if (parsed.origin !== window.location.origin || !parsed.pathname.startsWith('/api/')) {
      return originalFetch!(input as RequestInfo, init)
    }

    // App-specific special routes (SSE stream, raw binary, extracted text) — the
    // first match handles the request before any data-state transform.
    for (const sr of specialRoutes) {
      if (sr.test(parsed.pathname, method)) {
        return sr.respond({ path: parsed.pathname, method, mode: activeMode })
      }
    }

    // Apply the data-state mode. GET reads carry the state; mutations (POST/PUT/
    // DELETE) pass through so overlay forms can still "submit" against loaded data.
    const isRead = method === 'GET'
    const exempt = errorModeExempt.some(rx => rx.test(parsed.pathname))
    if (isRead && !exempt) {
      if (activeMode === 'error') {
        return mockErrorResponse(500)
      }
      if (activeMode === 'delayed') {
        await new Promise(r => setTimeout(r, DELAY_MS))
      }
    }

    const matched = matchRoute(method, parsed.pathname)
    if (!matched) {
      if (import.meta.env.DEV) {
        console.warn(`[gallery mockApi] no route for ${method} ${parsed.pathname}`)
      }
      return jsonResponse(makeSafeEmpty())
    }

    const entry = activeCassette[matched.route.key]
    if (entry === undefined) {
      if (import.meta.env.DEV) {
        console.warn(
          `[gallery mockApi] no cassette for ${matched.route.key} (${method} ${parsed.pathname})`,
        )
      }
      return jsonResponse(makeSafeEmpty())
    }

    const query: Record<string, string> = {}
    parsed.searchParams.forEach((v, k) => {
      query[k] = v
    })

    let body: unknown
    if (init?.body && typeof init.body === 'string') {
      try {
        body = JSON.parse(init.body)
      } catch {
        body = init.body
      }
    }

    const ctx: MockRequestContext = {
      params: matched.params,
      query,
      body,
      method,
    }
    let value =
      typeof entry === 'function'
        ? (entry as (c: MockRequestContext) => unknown)(ctx)
        : entry
    // `empty` mode: return a valid, well-shaped empty response (arrays emptied,
    // counts zeroed) — the state where "no data yet" bugs live.
    if (isRead && !exempt && activeMode === 'empty') {
      value = toEmpty(value)
    }
    return jsonResponse(value)
  }
}

/** Restore the original fetch (used by tests / teardown). */
export function uninstallMockApi(): void {
  if (originalFetch) globalThis.fetch = originalFetch
  installed = false
}
