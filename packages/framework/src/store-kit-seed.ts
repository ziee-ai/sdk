/**
 * The STORE SEED environment — the store-kit's server-side-rendering seam.
 *
 * Two directions across one document, and nothing domain-specific:
 *
 *   IN   a store declaring `seeded` merges its slice from the incoming
 *        envelope into its BIRTH state, before `createStore` ever runs.
 *   OUT  a store declaring `ssrExpose` registers a getter; a server render
 *        calls {@link collectStoreSeed} once it has quiesced, and the result is
 *        what the next document carries.
 *
 * ── Why a GLOBAL and not an injected provider ───────────────────────────────
 * The read direction is ordering-critical in a way an injected provider cannot
 * satisfy: `defineStore(...)` runs at MODULE SCOPE, so the seed has to be
 * readable before the first store module evaluates — which is before any app
 * bootstrap code could run to inject anything. A host writes
 * {@link STORE_SEED_GLOBAL} into the JS realm before the bundle is evaluated
 * (on a server), or a document-scoped script does (in a browser). The write
 * direction has no such constraint, so it is an ordinary module-scope registry
 * and needs no global at all.
 *
 * ── Why the framework may own this at all ───────────────────────────────────
 * Nothing here names an app type, a route, a permission or a domain entity: the
 * envelope is `Record<string, unknown>` keyed by STORE NAME, which `defineStore`
 * already owns. That is the same boundary `app-seam.ts` draws — the framework
 * supplies the mechanism and the consumer supplies the meaning.
 */

/** Where a host hands the incoming seed to the store layer. */
export const STORE_SEED_GLOBAL = '__ZIEE_STORE_SEED__'

/** The incoming envelope: store name → that store's serialized slice. */
export type StoreSeedEnvelope = Record<string, unknown>

interface SeedGlobals {
  [STORE_SEED_GLOBAL]?: unknown
}

/**
 * The incoming envelope, or `{}`.
 *
 * A FUNCTION, never a module-scope constant: a module-scope read would capture
 * whatever was set at the moment this file was evaluated, and on a server that
 * is decided by the bundler's module order rather than by the host. (The same
 * trap `renderMode.ts` documents for `__MANGWA_SSR__`, where a captured `false`
 * silently reduced a page from five seeded requests to one.)
 */
export function storeSeedEnvelope(): StoreSeedEnvelope {
  const raw = (globalThis as SeedGlobals)[STORE_SEED_GLOBAL]
  if (!raw || typeof raw !== 'object') return {}
  return raw as StoreSeedEnvelope
}

/** Install an incoming envelope. The app's hydration entry calls this. */
export function installStoreSeed(envelope: StoreSeedEnvelope | null | undefined): void {
  ;(globalThis as SeedGlobals)[STORE_SEED_GLOBAL] = envelope ?? {}
}

/**
 * How a store declares that its slice arrives from a server.
 *
 * `true` is a shallow spread of the incoming slice over the declared defaults.
 * That merge direction is the whole point (see the note on `applyStoreSeed`),
 * and a function form is offered for the store that needs a different one.
 */
export type StoreSeedOption<State> =
  | boolean
  | ((incoming: unknown, defaults: State) => State)

/** How a store declares which part of itself travels to the next document. */
export type StoreExposeOption<State> = boolean | ((state: State) => unknown)

/**
 * Merge a store's incoming slice into its birth state.
 *
 * **Spread, never replace.** A slice arrives through `JSON.stringify`, which
 * DROPS a key whose value is `undefined` — and several stores initialise a field
 * to exactly that. A replacing merge would delete those fields rather than leave
 * them at their declared default, so the store would be born missing keys its
 * own actions assume exist.
 *
 * Total by construction: a malformed slice (a string, an array, `null`) yields
 * the defaults. A seed is data from outside the process; it may not throw at
 * module evaluation, because a throw there takes the whole module graph down and
 * the app never boots.
 */
export function applyStoreSeed<State extends object>(
  name: string,
  defaults: State,
  option: StoreSeedOption<State> | undefined,
): State {
  if (!option) return defaults
  const incoming = storeSeedEnvelope()[name]
  if (typeof option === 'function') {
    try {
      return option(incoming, defaults)
    } catch {
      return defaults
    }
  }
  if (incoming === undefined || incoming === null) return defaults
  if (typeof incoming !== 'object' || Array.isArray(incoming)) return defaults
  return { ...defaults, ...(incoming as Partial<State>) }
}

/** name → the getter that produces this store's outgoing slice. */
const exposed = new Map<string, () => unknown>()

/**
 * Register a store's outgoing slice.
 *
 * Registration is at `defineStore` time — i.e. at module scope, before the store
 * has any state worth sending — so what is stored is a GETTER, read later. Last
 * registration wins for a given name, which matters only under HMR; two live
 * stores sharing a name is already a defect the proxy registry refuses.
 */
export function registerStoreExpose(name: string, getter: () => unknown): void {
  exposed.set(name, getter)
}

/**
 * Every exposing store's current slice, keyed by name — the payload a server
 * render hands to the document.
 *
 * A getter that throws is SKIPPED, loudly, rather than failing the render: one
 * store with a bad selector must not cost the whole page. The skipped name is
 * absent from the result, which the client reads as "not seeded" and handles by
 * falling back to its declared defaults — the same outcome as a store that never
 * declared `ssrExpose`.
 */
export function collectStoreSeed(): StoreSeedEnvelope {
  const out: StoreSeedEnvelope = {}
  for (const [name, getter] of exposed) {
    try {
      const value = getter()
      if (value !== undefined) out[name] = value
    } catch (err) {
      console.error(`[store-kit] ssrExpose getter for "${name}" threw; slice omitted`, err)
    }
  }
  return out
}

/** Names that have declared `ssrExpose`, whether or not they currently yield a
 *  slice. The gate and the tests read this; nothing at runtime depends on it. */
export function exposedStoreNames(): string[] {
  return [...exposed.keys()].sort()
}

/** TEST ONLY: drop every registration and the installed envelope. */
export function __resetStoreSeedForTests(): void {
  exposed.clear()
  delete (globalThis as SeedGlobals)[STORE_SEED_GLOBAL]
}

// ============================================================================
// The per-request `persist` storage.
// ============================================================================

/**
 * Where a host declares "this realm is producing a server render".
 *
 * A SECOND global next to the consuming app's own render-mode flag, and
 * deliberately so: the framework may not read a flag named after one app, and an
 * app may not be forced to name its flag after the framework. They are ONE FACT
 * with two readers, set together by whatever boots the render (in comic, the
 * host's `shim/00-mode.js`), and the pairing is stated at both ends so neither
 * can drift silently.
 */
export const SERVER_RENDER_GLOBAL = '__ZIEE_SERVER_RENDER__'

interface ServerRenderGlobals {
  [SERVER_RENDER_GLOBAL]?: boolean
}

/** True only inside a server render. A function call, never a captured const. */
export function isStoreServerRender(): boolean {
  return (globalThis as ServerRenderGlobals)[SERVER_RENDER_GLOBAL] === true
}

/** TEST/HOST ONLY: enter or leave server-render mode for the store layer. */
export function setStoreServerRender(on: boolean): void {
  if (on) (globalThis as ServerRenderGlobals)[SERVER_RENDER_GLOBAL] = true
  else delete (globalThis as ServerRenderGlobals)[SERVER_RENDER_GLOBAL]
}

/** The subset of zustand's `PersistStorage` shape `createJSONStorage` produces. */
export interface StringStorage {
  getItem: (name: string) => string | null
  setItem: (name: string, value: string) => void
  removeItem: (name: string) => void
}

/**
 * A fresh, in-memory, per-realm storage for one server render.
 *
 * **This OVERRIDES whatever the store declared — it does not fill an absent
 * one.** Filling only the absent ones is the trap: two of this app's five
 * persisted stores already ship their own `safeStorage`, whose `catch` on a
 * server falls back to a MODULE-SCOPE `Map`. In a realm that outlives one
 * request — a pooled context, or the recorded sidecar fallback where a single
 * V8 isolate serves many renders — that `Map` is one visitor's persisted state
 * handed to the next. Overriding is the only version of this that is safe
 * independently of the host's isolation model, which is the point: the property
 * belongs to the store layer, not to whichever engine happens to be underneath.
 *
 * A `Map` per call, closed over: two stores in one render share nothing, and
 * two renders share nothing even in one realm, because construction is what
 * makes the storage.
 */
export function createServerPersistStorage(): StringStorage {
  const cell = new Map<string, string>()
  return {
    getItem: name => cell.get(name) ?? null,
    setItem: (name, value) => {
      cell.set(name, value)
    },
    removeItem: name => {
      cell.delete(name)
    },
  }
}
