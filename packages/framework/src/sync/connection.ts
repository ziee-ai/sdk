// Module-level holder for the current realtime-sync SSE connection id.
//
// The SyncClient sets this on each (re)connect (from the server's
// `connected` handshake frame) and clears it on disconnect. The
// api-client chokepoint reads it to stamp `X-Sync-Connection-Id` on
// mutating requests, so the server can skip echoing a change back to the
// tab that made it (self-echo suppression). Kept in its own tiny module
// to avoid an import cycle between the api-client and the SyncClient.

let syncConnectionId: string | null = null

export const getSyncConnectionId = (): string | null => syncConnectionId

export const setSyncConnectionId = (id: string | null): void => {
  syncConnectionId = id
}
