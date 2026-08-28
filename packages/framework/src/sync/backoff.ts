// ============================================================================
// Realtime-sync reconnect backoff.
//
// Kept in its own dependency-free module (like `api-client/inflight.ts` and
// `lazy-dispatch.ts`) so the policy is unit-testable without SyncClient's fetch
// / EventBus graph.
// ============================================================================

export const INITIAL_BACKOFF_MS = 1_000
export const MAX_BACKOFF_MS = 30_000

// A 429 is a CAPACITY refusal (the per-user SSE connection cap), not a transient
// drop: retrying at the 1 s transient floor just burns requests against an
// endpoint that has already said "no room" — the network audit sees it as 2–3
// duplicate `/api/sync/subscribe` per page load. Back off an order of magnitude
// further, and JITTER it so N tabs/devices refused at once don't re-attempt in
// lockstep and re-collide. Still bounded by MAX_BACKOFF_MS, so this is never
// slower than the existing worst case.
export const CAPACITY_BACKOFF_MS = 10_000
export const CAPACITY_JITTER_MS = 5_000

/**
 * Delay before the next subscribe attempt.
 *
 * A capacity refusal raises the FLOOR; it never lowers the delay. The loop's own
 * `currentBackoffMs` keeps escalating in the background, so repeated 429s still
 * ramp toward `MAX_BACKOFF_MS` exactly as repeated transient failures do —
 * taking the MAX of the two is what makes "back off further" true in both
 * directions. Returning the capacity floor alone would, once the transient
 * ladder had escalated to 30 s, actually SHORTEN the wait to ~12 s and then
 * never escalate again: retrying FASTER against an endpoint that just said "no
 * room", the exact opposite of the intent.
 *
 * @param status  HTTP status that refused the stream, or `null` for a transient
 *                failure (socket drop, DNS blip, an aborted body).
 * @param currentBackoffMs  the loop's escalating transient backoff.
 */
export function reconnectDelayMs(
  status: number | null,
  currentBackoffMs: number,
  rand: () => number = Math.random,
): number {
  const capacityFloor =
    status === 429
      ? CAPACITY_BACKOFF_MS + Math.floor(rand() * CAPACITY_JITTER_MS)
      : 0
  return Math.min(Math.max(currentBackoffMs, capacityFloor), MAX_BACKOFF_MS)
}
