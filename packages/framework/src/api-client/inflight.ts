// ============================================================================
// In-flight GET coalescing.
//
// Two callers that issue the SAME read while a request is already on the wire
// share ONE round-trip instead of two. This is a de-duplicator, deliberately
// NOT a cache: an entry exists only while its request is unsettled, and is
// dropped the moment it resolves or rejects. A joiner can therefore never
// receive anything it could not equally have received by arriving 1 ms earlier.
//
// WHY here and not in each store: the store-level `if (state.loading) return`
// guards cannot fire in time. store-kit dispatches a lazy action as
// `resolveImpl().then(impl => impl(...args))`, so the action body — and its
// guard — only runs after the action's chunk resolves; two synchronous callers
// both slip past every guard. Coalescing at the single transport chokepoint
// fixes that for every store at once (and for the callers that have no guard at
// all).
//
// ── The staleness guard (the load-bearing part) ─────────────────────────────
// A naive key→promise map is unsafe: a component that mutates and then refetches
// could JOIN a read that started BEFORE its mutation and render pre-mutation
// data — a stale refetch, which is a worse defect than the duplicate request it
// saved. So every entry records the freshness EPOCH it started in, and a joiner
// may only join an entry whose epoch is still current.
//
// `bumpFetchEpoch()` is called on:
//   - every completed non-GET request (a local mutation), and
//   - every inbound realtime-sync frame (a remote mutation).
//
// After either, in-flight reads are "possibly pre-change" and are no longer
// joinable, so the notify-and-refetch contract (every store refetches on its
// `sync:<entity>` / `sync:reconnect`) still performs a genuine round-trip.
//
// The epoch/generation idiom is the house pattern for exactly this race — see
// `sync/SyncClient.ts`'s `epoch` (a user switch supersedes an in-flight loop)
// and `chatHistory/actions/loadRecentConversations.ts`'s `recentLoadSeq` (a
// mid-flight reset discards a stale page).
// ============================================================================

interface Entry {
  /** The unsettled request. Removed from the map as soon as it settles. */
  promise: Promise<unknown>
  /** The freshness epoch this request STARTED in. */
  epoch: number
  /** When it was issued — see MAX_JOIN_AGE_MS. */
  startedAt: number
}

/**
 * A read older than this is no longer joinable, even in the same epoch.
 *
 * The transport sets no fetch timeout, so a socket that HANGS never settles and
 * its entry would otherwise stay in the map forever — and on a quiet page (no
 * mutations, no sync frames) the epoch never moves either, so every later
 * identical read would join a dead promise and that endpoint would be
 * permanently unreadable for the session. Before coalescing, each caller got its
 * own attempt and a remount recovered. This bound restores that: the hung
 * request is still awaited by whoever issued it, but a later caller opens a
 * fresh one.
 *
 * Sizing is a JUDGEMENT, not a derivation: the transport's retry ladder sleeps
 * ~6 s in total, but the six `fetch()` attempts themselves are UNTIMED (nothing
 * in this transport sets a fetch or connect timeout), so a slow-connect ladder or
 * a multi-MB Blob download can exceed this bound. When it does, the only cost is
 * a MISSED join — the late caller issues its own request, which is exactly the
 * pre-coalescing behaviour. That is the safe direction, which is why the bound is
 * generous rather than tight.
 *
 * KNOWN, UNFIXED HERE: a caller arriving INSIDE the window of a hung request
 * inherits that hung wait instead of getting its own attempt. The real fix is a
 * fetch timeout on the transport, which this branch does not add (it would change
 * behaviour for every request in three applications, well beyond this feature).
 */
export const MAX_JOIN_AGE_MS = 15_000

const inFlight = new Map<string, Entry>()

/** Monotonic freshness counter. Bumped by any event that can change what a read
 *  would return; an in-flight read from an older epoch is no longer joinable. */
let fetchEpoch = 0

/**
 * Invalidate joinability of every in-flight read. Call after a local mutation
 * completes or when a realtime-sync frame arrives. Cheap (an integer bump) — it
 * cancels nothing and aborts nothing; already-issued requests still settle for
 * the caller that started them.
 */
export function bumpFetchEpoch(): void {
  fetchEpoch++
}

/** Current epoch. Exported for tests and for callers that need to detect
 *  "did anything change while I was awaiting?" (e.g. a freshness window). */
export function currentFetchEpoch(): number {
  return fetchEpoch
}

/**
 * Non-cryptographic 32-bit fingerprint (FNV-1a). Used to fold the auth token
 * into the coalescing key WITHOUT putting the token itself in a map key that
 * could surface in a log or an error message.
 */
function fingerprint(value: string): string {
  let h = 0x811c9dc5
  for (let i = 0; i < value.length; i++) {
    h ^= value.charCodeAt(i)
    h = Math.imul(h, 0x01000193)
  }
  return (h >>> 0).toString(36)
}

/**
 * Build the coalescing key for a read, folding in a fingerprint of the caller's
 * token so a login / user switch cannot let one identity join a read issued under
 * another. The fingerprint is a 32-bit FNV-1a fold, so this is a strong practical
 * separation rather than a cryptographic guarantee; the token itself is never put
 * in a map key, which could surface in a log or an error message.
 *
 * NOTE the token is read here and read again in the request builder, so a
 * rotation between the two means the key records the credential at KEY time, not
 * necessarily the one the request carried. Harmless for a same-user rotation (the
 * response is for the same principal either way) and a cross-user switch changes
 * the fingerprint, which is the case that matters.
 */
export function inflightKey(
  method: string,
  urlWithQuery: string,
  token: string | null,
): string {
  return `${method} ${urlWithQuery} @${token ? fingerprint(token) : 'anon'}`
}

/**
 * Run `start()` under `key`, or JOIN an equivalent request already in flight in
 * the CURRENT epoch.
 *
 * Returns the shared promise. All joiners observe the same resolution — and the
 * same rejection — as the caller that actually issued the request, so a 401
 * refresh-and-retry or a transient-GET retry ladder inside `start()` happens
 * ONCE for the whole group rather than N times.
 */
export function coalesce<T>(key: string, start: () => Promise<T>): Promise<T> {
  const now = Date.now()
  const existing = inFlight.get(key)
  if (existing && !isJoinable(existing, now)) {
    // Evict it here rather than only refusing to join. Cleanup normally happens
    // in the request's own `.finally()`, which by definition never runs for a
    // socket that hangs — so without this the Map would retain one permanent
    // entry per distinct hung read for the page's lifetime.
    inFlight.delete(key)
  }
  if (existing && isJoinable(existing, now)) {
    // ALIASING GUARD: the issuer keeps the response it parsed; a joiner gets its
    // OWN copy. Without this, N callers would share one parsed object/array
    // instance, and a store that normalizes its response in place (`.sort()`,
    // `.push()`, an immer draft assigning the same reference) would silently
    // corrupt what every other joiner sees — a class of bug that did not exist
    // when each caller performed its own parse. `structuredClone` is only paid
    // when a join actually happens, which is the rare case.
    return (existing.promise as Promise<T>).then(isolate)
  }

  const epoch = fetchEpoch
  const promise = start().finally(() => {
    // Only clear OUR entry: a later caller from a newer epoch may have replaced
    // it, and dropping that one would leave its joiners orphaned in the map.
    if (inFlight.get(key)?.promise === promise) inFlight.delete(key)
  })
  inFlight.set(key, { promise, epoch, startedAt: now })
  return promise
}

/** Joinable = same freshness epoch AND young enough (see MAX_JOIN_AGE_MS). */
function isJoinable(entry: Entry, now: number): boolean {
  return entry.epoch === fetchEpoch && now - entry.startedAt < MAX_JOIN_AGE_MS
}

/** Deep-copy a joined response so joiners never alias the issuer's object.
 *
 *  A non-cloneable payload now THROWS rather than falling back to the shared value.
 *  The fallback was a silent-degradation trap: a `ReadableStream` cannot be cloned, so
 *  two joiners would receive the SAME body, the first `getReader()` would lock it and
 *  the second would fail with an unrelated `TypeError: locked` — far from the cause,
 *  with nothing naming it. Failing here names it exactly once, at the seam that knows.
 *
 *  This is unreachable on every path that exists today: the parse switch yields JSON
 *  (cloneable), a string (primitive, returned above) or a `Blob` (cloneable), and a
 *  `responseType: 'stream'` call is excluded from joining by the predicate in
 *  `core.ts`. It is the backstop for the case where that exclusion is ever weakened —
 *  a guard, not a behaviour change.
 */
function isolate<T>(value: T): T {
  if (value === null || typeof value !== 'object') return value
  try {
    return structuredClone(value)
  } catch (cause) {
    // The cause is folded into the MESSAGE rather than passed as `{ cause }`: this
    // package's TS lib target predates the two-argument `Error` constructor, and the
    // transport is shared by three applications — narrowing their lib floor to carry a
    // diagnostic would be a real cost for no gain.
    const why = cause instanceof Error ? cause.message : String(cause)
    throw new Error(
      'api-client: a coalesced response could not be isolated for a joined caller ' +
        `(${why}). The value is not structured-cloneable — a ReadableStream or Response ` +
        'body is not — and sharing it would give two callers ONE body: the first reader ' +
        'locks it and the second fails somewhere else entirely. Mark this call ' +
        "`responseType: 'stream'` (excluded from coalescing) or `noCoalesce: true`.",
    )
  }
}

/** Test seam: drive {@link isolate} directly.
 *
 *  The hard-fail it guards is UNREACHABLE through the public paths — the parse switch
 *  yields JSON / a string / a Blob, and a `responseType: 'stream'` call is excluded from
 *  joining — so the only way to assert it is to call it. Without this seam the guard
 *  would be asserted by inference, which is how a guard rots into a comment. */
export function __isolateForTests<T>(value: T): T {
  return isolate(value)
}

/** Test seam: drop all in-flight bookkeeping. The epoch is BUMPED, never
 *  rewound — it is documented as monotonic, and rewinding it would let a
 *  previously-captured epoch (e.g. `meFreshness`'s) spuriously compare equal. */
export function __resetInflightForTests(): void {
  inFlight.clear()
  fetchEpoch++
}

/** Test/diagnostic seam: how many reads are currently joinable. */
export function inflightSize(): number {
  return inFlight.size
}
