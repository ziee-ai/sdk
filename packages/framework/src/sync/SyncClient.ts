import { getAuthToken, getBaseUrl } from '../api-client/core'
// Generic SSE frame shape (app-agnostic): the runtime only reads these
// three string fields off each 'sync' frame, so the framework needs no
// dependency on the app's generated SyncEntity/SyncAction unions.
interface SyncEvent { entity: string; action: string; id: string }
import { useEventBusStore } from '../events/store'
import { setSyncConnectionId } from './connection'

// Realtime-sync SSE client. A thin bridge: opens the per-user
// `GET /api/sync/subscribe` stream and re-emits each `{entity, action,
// id}` frame onto the client EventBus as a per-entity `sync:<entity>`
// event. Each store subscribes to its own `sync:<entity>` in its
// `__init__.__store__` (like any local event) and refetches. Reuses the
// same fetch + ReadableStream approach as the api-client so header auth
// works (EventSource can't set Authorization).

const INITIAL_BACKOFF_MS = 1_000
const MAX_BACKOFF_MS = 30_000
// A connection must stay alive this long before we trust it as "stable" and
// reset the reconnect backoff. A stream that connects then immediately drops
// (flapping) keeps backing off instead of hammering the 1s floor.
const STABLE_AFTER_MS = 3_000
// Debounce the reconnect resync: emit `sync:reconnect` (which reloads EVERY
// store) at most this often, so a flapping stream can't drive a continuous
// all-stores reload storm — which would itself overload the connection and
// keep it flapping (a self-reinforcing loop).
const RESYNC_MIN_INTERVAL_MS = 5_000

let started = false
// Monotonic loop generation. A user-switch (stop→start) bumps this so the
// previous loop — which may be suspended in `reader.read()` — supersedes
// itself instead of opening a SECOND concurrent stream under the new identity.
let epoch = 0
let activeAbort: AbortController | null = null
let backoffMs = INITIAL_BACKOFF_MS
// The first connect needs no resync (stores load on init); only RE-connects
// after real downtime must catch missed events.
let hasConnectedOnce = false
let lastResyncAt = 0

/** Start the sync stream (idempotent). Call when a user is authenticated. */
export function startSyncClient(): void {
  if (started) return
  started = true
  backoffMs = INITIAL_BACKOFF_MS
  hasConnectedOnce = false
  lastResyncAt = 0
  const myEpoch = ++epoch
  void connectLoop(myEpoch)
}

/** Stop the sync stream and clear the connection id. Call on logout /
 *  user-switch. Bumps the epoch so any running loop bails. */
export function stopSyncClient(): void {
  started = false
  epoch++
  activeAbort?.abort()
  activeAbort = null
  setSyncConnectionId(null)
}

async function connectLoop(myEpoch: number): Promise<void> {
  while (started && myEpoch === epoch) {
    try {
      await connectOnce(myEpoch)
    } catch (error) {
      if (!started || myEpoch !== epoch) break
      if (!(error instanceof DOMException && error.name === 'AbortError')) {
        console.warn('[sync] stream ended; reconnecting', error)
      }
    }
    if (!started || myEpoch !== epoch) break
    await delay(backoffMs)
    backoffMs = Math.min(backoffMs * 2, MAX_BACKOFF_MS)
  }
}

async function connectOnce(myEpoch: number): Promise<void> {
  const token = getAuthToken()
  if (!token) {
    // No token while "started" = the session lapsed (token expiry) without a
    // logout. Quietly back off and retry rather than throwing/warning each
    // cycle — a token refresh re-enables us on the next attempt.
    return
  }

  const baseUrl = await getBaseUrl()
  // `getBaseUrl` awaits (a dynamic-port lookup on desktop/Tauri) — if a
  // user-switch superseded this loop meanwhile, bail before opening a stream.
  if (!started || myEpoch !== epoch) return

  const abort = new AbortController()
  activeAbort = abort

  const response = await fetch(`${baseUrl}/api/sync/subscribe`, {
    headers: {
      Authorization: `Bearer ${token}`,
      Accept: 'text/event-stream',
    },
    signal: abort.signal,
  })

  // Superseded while the fetch was in flight → abort this stream + bail so we
  // don't clobber the new loop's `activeAbort` or leak a reader.
  if (!started || myEpoch !== epoch) {
    abort.abort()
    return
  }

  if (!response.ok || !response.body) {
    throw new Error(`[sync] subscribe failed: ${response.status}`)
  }

  // Established. Reset the reconnect backoff only AFTER the stream proves
  // stable (survives `STABLE_AFTER_MS`); a stream that connects then drops
  // immediately keeps backing off instead of hammering the 1s floor.
  const stabilityTimer = globalThis.setTimeout(() => {
    backoffMs = INITIAL_BACKOFF_MS
  }, STABLE_AFTER_MS)

  // Resync to cover events missed while disconnected — skipped on the first
  // connect and debounced (see `maybeResync`).
  maybeResync()

  const reader = response.body.getReader()
  const decoder = new globalThis.TextDecoder()
  let buffer = ''
  let currentEvent = ''

  try {
    while (started && myEpoch === epoch) {
      const { done, value } = await reader.read()
      if (done) break

      buffer += decoder.decode(value, { stream: true })
      const lines = buffer.split(/\r\n|\n/)
      buffer = lines.pop() || ''

      for (const line of lines) {
        if (line.trim() === '') {
          currentEvent = ''
          continue
        }
        if (line.startsWith('event: ')) {
          currentEvent = line.slice(7).trim()
        } else if (line.startsWith('data: ')) {
          const raw = line.slice(6)
          let parsed: unknown = raw
          try {
            parsed = JSON.parse(raw)
          } catch {
            // keep as string
          }
          handleFrame(currentEvent, parsed)
        }
      }
    }
  } finally {
    globalThis.clearTimeout(stabilityTimer)
  }
}

// Emit the `sync:reconnect` resync signal, but only for GENUINE reconnects:
// never on the first connect (stores load on init), and never more often than
// `RESYNC_MIN_INTERVAL_MS` so a flapping stream can't storm every store.
function maybeResync(): void {
  if (!hasConnectedOnce) {
    hasConnectedOnce = true
    return
  }
  const now = Date.now()
  if (now - lastResyncAt < RESYNC_MIN_INTERVAL_MS) return
  lastResyncAt = now
  void useEventBusStore.getState().emit({ type: 'sync:reconnect', data: {} } as never)
}

function handleFrame(event: string, data: unknown): void {
  if (event === 'connected') {
    const connId = (data as { connection_id?: string } | null)?.connection_id
    if (typeof connId === 'string') {
      setSyncConnectionId(connId)
    }
    return
  }

  if (event === 'sync') {
    const ev = data as SyncEvent | null
    if (!ev || !ev.entity || !ev.action || !ev.id) return
    // Re-emit onto the existing EventBus as a per-entity event; whichever
    // store subscribed to this `sync:<entity>` reacts. Cast: the
    // template-literal key is a valid `keyof AppEvents` but TS can't narrow
    // it from the runtime entity string.
    void useEventBusStore.getState().emit({
      type: `sync:${ev.entity}`,
      data: { action: ev.action, id: ev.id },
    } as never)
  }
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => globalThis.setTimeout(resolve, ms))
}
