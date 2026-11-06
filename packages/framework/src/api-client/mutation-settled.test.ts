import { test } from 'node:test'
import assert from 'node:assert/strict'
import { callAsync, setBaseUrlResolver } from './core.ts'
import { currentFetchEpoch, onMutationSettled } from './inflight.ts'

// The per-domain freshness input (history-pinned-stores): every SETTLED
// non-GET through the transport tells its observers WHICH endpoint was
// written to, at the same moment the global epoch bumps; a GET tells nobody.

test('MUT-1: a settled mutation names its endpoint to the observers (success and failure); a GET does not', async () => {
  setBaseUrlResolver(async () => 'http://127.0.0.1:9')
  const seen: string[] = []
  const off = onMutationSettled(e => seen.push(e))
  const realFetch = globalThis.fetch
  let status = 200
  globalThis.fetch = (async () =>
    new Response(JSON.stringify({ ok: true }), {
      status,
      headers: { 'content-type': 'application/json' },
    })) as typeof fetch
  try {
    const e0 = currentFetchEpoch()
    await callAsync('POST /api/forum/communities/{slug}/posts', { slug: 'g' }).catch(() => undefined)
    assert.deepEqual(seen, ['POST /api/forum/communities/{slug}/posts'])
    assert.ok(currentFetchEpoch() > e0)
    status = 500
    await callAsync('DELETE /api/reader/progress/{id}', { id: 'x' }).catch(() => undefined)
    assert.equal(seen[seen.length - 1], 'DELETE /api/reader/progress/{id}')
    status = 200
    const n = seen.length
    await callAsync('GET /api/catalog/series/{slug}', { slug: 'a' }).catch(() => undefined)
    assert.equal(seen.length, n)
    off()
    await callAsync('POST /api/forum/votes', {}).catch(() => undefined)
    assert.equal(seen.length, n, 'unsubscribed')
  } finally {
    globalThis.fetch = realFetch
  }
})
