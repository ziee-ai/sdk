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
}

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
 * Build the coalescing key for a read. Includes the identity fingerprint so a
 * login / user switch mid-flight can never hand user B a response fetched as
 * user A.
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
  const existing = inFlight.get(key)
  if (existing && existing.epoch === fetchEpoch) {
    return existing.promise as Promise<T>
  }

  const epoch = fetchEpoch
  const promise = start().finally(() => {
    // Only clear OUR entry: a later caller from a newer epoch may have replaced
    // it, and dropping that one would leave its joiners orphaned in the map.
    if (inFlight.get(key)?.promise === promise) inFlight.delete(key)
  })
  inFlight.set(key, { promise, epoch })
  return promise
}

/** Test seam: drop all in-flight bookkeeping and reset the epoch. */
export function __resetInflightForTests(): void {
  inFlight.clear()
  fetchEpoch = 0
}

/** Test/diagnostic seam: how many reads are currently joinable. */
export function inflightSize(): number {
  return inFlight.size
}
