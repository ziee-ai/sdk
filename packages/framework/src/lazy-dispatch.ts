// ============================================================================
// Lazy-action dispatch — extracted from store-kit so it can be reasoned about
// (and unit-tested) on its own, with no zustand/EventBus dependency graph.
//
// A lazy action lives in its own chunk: calling it must (1) download the chunk,
// (2) build the impl from `(set, get)`, then (3) invoke it. The chunk + impl are
// memoized so the download happens once; `preload()` warms them without
// invoking the action.
//
// ── Why the loader is passed as TWO stages ──────────────────────────────────
// The two stages fail for completely different reasons and must be recovered
// from differently:
//
//   - **import** (`() => import('./actions/foo')`) fails for TRANSIENT,
//     environmental reasons: a network blip, a proxy hiccup, or — the common one
//     in production — ANY deploy while a tab is open, which invalidates the
//     hashed chunk URLs the already-loaded page still holds. Retrying is
//     correct, and adding our OWN permanent memo on top is actively harmful.
//   - **build** (`mod => mod.default(set, get)`) fails only for DETERMINISTIC
//     reasons: the action factory itself threw, i.e. an authoring bug. Retrying
//     that forever would turn one bug into an unbounded loop for a component
//     that dispatches from a render or an effect.
//
// A single combined loader cannot tell the two apart (short of sniffing error
// messages, which is unreliable across browsers and bundlers), so the previous
// one-argument form conflated them and applied the DETERMINISTIC policy to both:
// one retry, then the rejection was memoized permanently. A live-UI audit caught
// the consequence — one transient blip permanently bricked the chat model
// picker's lazy action, and every subsequent send then posted a body missing
// `model_id` and got a raw 422.
//
// ── HONEST LIMIT, and how issue #161 / FUP-65 narrows it ────────────────────
// Retrying does NOT make every chunk failure self-heal in a browser on its
// own. Per the HTML spec the module map records a FAILED fetch for a URL, so
// re-`import()`ing the SAME bundler-rewritten specifier fails again WITHOUT
// re-requesting, for the life of that document (measured directly in this
// repo's e2e: 9 import attempts produced only 2 network requests) — and the
// ORIGINAL specifier is bundler-generated, fixed at build time, and not ours
// to alter from here.
//
// What plain retry recovers on its own:
//   - a Vite `__vitePreload` DEP failure — the helper re-creates its
//     `<link rel=modulepreload>` on every call, so a link that 503s once can
//     succeed on the next attempt;
//   - any loader that is not a module-map-backed `import()` (a fetch-based
//     loader, a dev-server round trip, a test double);
//   - a failure that happens BEFORE the fetch is recorded (an aborted request
//     that never reached the map).
//
// A hard 404/aborted fetch of the ORIGINAL static specifier does NOT recover
// on its own — but issue #161 (FUP-65) is the one case worth going further
// for: an on-demand click made OFFLINE fails, poisons that one chunk, and
// stayed broken after reconnecting until a full reload, because every retry
// kept hitting the SAME now-poisoned specifier. `importWithRetry` below closes
// that gap WITHOUT altering the original specifier (still not ours to touch):
// the browser's own rejection for a failed dynamic import carries the
// resolved URL in its message ("Failed to fetch dynamically imported module:
// <url>" in Chromium); once a first attempt has failed and that URL is
// recovered from the error, EVERY later attempt — this call's own remaining
// retries, or a wholly separate later dispatch — re-fetches a CACHE-BUSTED
// variant of that URL (`<url>?ziee_retry=N`) via `importUrl` instead of
// calling `importModule()` again. A query-suffixed URL is a specifier the
// module map has never seen, so it gets a genuinely fresh fetch rather than
// the cached failure — while the FIRST-EVER attempt is always the real
// `importModule()` call, offline or not, so a deliberate offline probe still
// observes the browser's own native failure. Bounded by the same
// `MAX_IMPORT_RETRIES`/backoff as ever, and — "never loop while offline" —
// once a URL has already been recovered, a further attempt is skipped
// outright while `isOffline()`, rather than spending the backoff on a
// request that cannot succeed. A hard 404 (a chunk genuinely gone after a
// deploy) still fails the same way through this path — the cache-busted URL
// 404s too — so the give-up path below (mark build stale, tell the user to
// reload) is unchanged for that case.
//
// ── Why this does NOT de-duplicate calls ────────────────────────────────────
// There IS a real hole here: the action BODY — and therefore the action's own
// in-flight guard (`if (state.loading) return`) — cannot run until the chunk
// resolves, so two callers in the same tick both get past every guard. It is
// tempting to close that by merging same-argument calls made during the
// chunk-load window. That was tried and REJECTED, because a dispatcher cannot
// tell a read from a mutation:
//
//   - merging is applied to EVERY lazy action, so two deliberate identical
//     mutations issued in that window (a double-clicked create, two components
//     each dispatching `markRead(id)`) would collapse into one invocation and
//     BOTH callers would resolve successfully — a silently dropped intent;
//   - `JSON.stringify` does NOT throw on a function/Map/Set/class instance, it
//     emits `null`/`{}`, so calls carrying DIFFERENT callbacks key identically
//     and the second callback is never invoked.
//
// The duplicate NETWORK requests those un-guarded calls produce are removed one
// layer down, at the transport (`api-client/inflight.ts`), where "is this a
// read?" is knowable (`GET`, non-SSE, non-upload) and a mutation is excluded by
// construction. Two cold `loadConversations()` calls therefore still both run
// their body — harmless, they set the same state — but issue ONE request.
// ============================================================================

import { clearStaleBuild, isOffline, markStaleBuild } from './chunk-recovery'

/** Stage 1: download the action's chunk. */
export type ModuleLoader<M> = () => Promise<M>

/** Stage 2: build the callable impl from the loaded module. */
export type ImplBuilder<M> = (mod: M) => (...args: any[]) => any

/** The callable a lazy action becomes on a store. Named for the ACTION it wraps
 *  (store-kit exports a differently-shaped, fully-typed `LazyDispatcher<L>` for
 *  the same concept at the type level — these must not share a name). */
export interface LazyActionDispatcher {
  (...args: any[]): Promise<any>
  /** Warm the chunk + build the impl without invoking it. */
  preload: () => Promise<void>
}

/**
 * How many times a failed impl BUILD is retried before the rejection is
 * memoized.
 *
 * A throw from the action FACTORY is a deterministic authoring bug — retrying it
 * forever would turn one bug into an unbounded loop for a component that
 * dispatches from a render or an effect. One retry covers a factory that
 * genuinely depended on a transient value; the second failure is treated as
 * deterministic and memoized, so the action fails fast and loudly from then on.
 */
const MAX_BUILD_RETRIES = 1

/**
 * How many EXTRA import attempts ONE dispatch makes before giving up.
 *
 * Bounded so a hard 404 (a chunk that is genuinely gone) still fails within a
 * few hundred milliseconds instead of hanging the caller. The rejection is never
 * memoized regardless of this budget, so a LATER dispatch always starts a fresh
 * one — that is what makes a transient blip self-healing rather than
 * session-fatal.
 */
const MAX_IMPORT_RETRIES = 2

/** Linear backoff between import attempts, in ms: 150 then 300. */
const IMPORT_RETRY_BACKOFF_MS = 150

const delay = (ms: number) =>
  new Promise<void>(resolve => setTimeout(resolve, ms))

/**
 * Best-effort extraction of the failing module's URL from a dynamic-import
 * rejection's message.
 *
 * Chromium's exact wording is "Failed to fetch dynamically imported module:
 * <url>" (the same phrase this repo's e2e guard/spec fixtures match on), but
 * other engines phrase it differently — so this looks for ANY absolute URL in
 * the message rather than anchoring on one engine's exact wording. Returns
 * `null` when no URL is recoverable (a build error, a non-network throw, or
 * an engine whose message this can't parse) — the caller then falls back to
 * the pre-existing plain-retry behaviour for that error.
 */
function extractModuleUrl(err: unknown): string | null {
  const message = err instanceof Error ? err.message : String(err)
  const match = message.match(/https?:\/\/\S+/)
  if (!match) return null
  return match[0].replace(/[)"'.,]+$/, '')
}

/** Default `importUrl`: a real dynamic import of a runtime-computed URL.
 *  `@vite-ignore` is required — without it Vite tries to statically analyse
 *  and rewrite the specifier, which defeats the whole point (the URL here is
 *  already a resolved, absolute, cache-busted variant, not a bundler-relative
 *  literal). */
function defaultImportUrl(url: string): Promise<unknown> {
  return import(/* @vite-ignore */ url)
}

export interface LazyDispatchOptions {
  /**
   * Sleep between import attempts. Defaults to a real `setTimeout`.
   *
   * Injectable purely as a TEST seam: the backoff constants are deliberately
   * tuned, and without this every failure-path spec would pay them in real wall
   * clock (~0.9-1.4s each) while asserting nothing about the timing.
   */
  sleep?: (ms: number) => Promise<void>
  /**
   * Import an already-RESOLVED url directly (the cache-busted retry path,
   * issue #161 / FUP-65). Defaults to a real `import()` — see
   * `defaultImportUrl`. TEST SEAM: a real dynamic import of a synthetic test
   * URL would try to hit the network (or throw oddly) under plain Node, so
   * specs inject a fake.
   */
  importUrl?: (url: string) => Promise<unknown>
}

/**
 * Build the lazy dispatcher for ONE action.
 *
 * @param importModule stage 1 — download the chunk. A rejection here is treated
 *        as TRANSIENT: retried with backoff, and never memoized.
 * @param buildImpl    stage 2 — build the callable from the loaded module. A
 *        throw here is treated as DETERMINISTIC: memoized after
 *        `MAX_BUILD_RETRIES`.
 */
export function createLazyDispatcher<M = any>(
  importModule: ModuleLoader<M>,
  buildImpl: ImplBuilder<M>,
  options: LazyDispatchOptions = {},
): LazyActionDispatcher {
  let implPromise: Promise<(...args: any[]) => any> | null = null
  let buildFailures = 0
  const sleep = options.sleep ?? delay
  const importUrl = options.importUrl ?? defaultImportUrl

  // ITEM-5b (issue #161 / FUP-65) state — persists ACROSS separate dispatch()
  // calls (unlike `lastError`, which is local to one `importWithRetry` run):
  // once a real attempt has failed and told us its URL, every later attempt —
  // this call's own remaining retries, or a wholly separate later dispatch —
  // re-fetches a cache-busted variant of THAT url instead of calling
  // `importModule()` again (which would just hit the same poisoned specifier).
  // `retryCount` makes each busted variant unique so the module map treats
  // every one as brand new.
  let poisonedUrl: string | null = null
  let retryCount = 0

  /** Import with bounded retry + linear backoff. Rejects with the LAST error. */
  const importWithRetry = async (): Promise<M> => {
    let lastError: unknown
    for (let attempt = 0; attempt <= MAX_IMPORT_RETRIES; attempt++) {
      // "Never loop while offline": once we already have a poisoned URL (this
      // call's own earlier attempt, or an earlier dispatch entirely), a
      // further attempt — original or cache-busted — is guaranteed to fail
      // while offline. Skip it outright rather than spending the backoff on a
      // doomed request. The FIRST-EVER attempt (poisonedUrl still null) always
      // goes through even while offline: it is what proves the failure (and
      // is where the URL to cache-bust later comes from) — a deliberate
      // offline probe must still see the browser's own native failure.
      if (poisonedUrl && isOffline()) {
        lastError = new Error(
          'lazy chunk import deferred — offline (will retry once back online)',
        )
        break
      }
      if (attempt > 0) await sleep(IMPORT_RETRY_BACKOFF_MS * attempt)
      try {
        const mod = (
          poisonedUrl
            ? await importUrl(
                `${poisonedUrl}${poisonedUrl.includes('?') ? '&' : '?'}ziee_retry=${++retryCount}`,
              )
            : await importModule()
        ) as M
        // A NULLISH module namespace means the import FAILED but something
        // swallowed the rejection — Vite's `__vitePreload` does exactly this
        // when a `vite:preloadError` listener calls `preventDefault()`:
        // `baseModule().catch(handlePreloadError)` then resolves with
        // `undefined`. Left alone it reaches `buildImpl`, blows up with a
        // confusing "Cannot read properties of undefined (reading 'default')",
        // and — worse — is misclassified as a DETERMINISTIC factory bug and
        // memoized forever. Classify it here as what it is: a failed import.
        if (mod == null) {
          throw new Error(
            'lazy chunk import resolved with no module — the load failed and something suppressed the rejection (e.g. a vite:preloadError listener calling preventDefault)',
          )
        }
        // A chunk just loaded: whatever failed earlier is no longer failing, so
        // the stale mark (which gates both the user-facing "the app may have been
        // updated" hint and store-kit's prefetch bail) must not outlive it.
        clearStaleBuild()
        return mod
      } catch (err) {
        lastError = err
        if (!poisonedUrl) poisonedUrl = extractModuleUrl(err)
      }
    }
    throw lastError
  }

  const resolveImpl = (): Promise<(...args: any[]) => any> => {
    if (implPromise) return implPromise
    implPromise = (async () => {
      let mod: M
      try {
        mod = await importWithRetry()
      } catch (err) {
        // TRANSIENT — clear the memo unconditionally so the user's NEXT action
        // re-downloads the chunk. This dispatcher must never ADD a permanent
        // memo of its own on top of whatever the platform already caches.
        implPromise = null
        // Record that a code-split chunk has definitively failed in this page's
        // lifetime. Vite's `vite:preloadError` listener marks the same flag, but
        // it only fires for `__vitePreload`-wrapped imports in a production
        // build — this covers dev, a plain `import()`, and the `mod == null`
        // classification above, so the user-facing message can explain WHY
        // reloading helps in those cases too.
        markStaleBuild()
        throw err
      }
      try {
        return buildImpl(mod)
      } catch (err) {
        // DETERMINISTIC — one retry, then memoize, so an authoring bug fails
        // fast and loudly instead of looping.
        buildFailures++
        if (buildFailures <= MAX_BUILD_RETRIES) implPromise = null
        throw err
      }
    })()
    return implPromise
  }

  const dispatch = ((...args: any[]) =>
    resolveImpl().then(impl => impl(...args))) as LazyActionDispatcher

  dispatch.preload = () => resolveImpl().then(() => undefined)
  return dispatch
}
