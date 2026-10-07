import { test } from 'node:test'
import assert from 'node:assert/strict'
import { substitutePathParams } from './path-substitution.ts'
import { callAsync, setBaseUrlResolver } from './core.ts'

// phibya/comic#502 — path-parameter substitution semantics, pinned for the
// ONE shared helper.
//
// The old SDK transport gated only on `params[c] !== undefined`, so a `null`
// capture silently became the literal path segment "null" while the app's SSR
// transport threw. The helper here is the single implementation both call;
// the semantics below are what both transports must agree on, and the app's
// `path-substitution-parity.test.ts` re-asserts the SAME table through both
// transports.
//
// Runs under `node:test` (purposely: pure logic, no DOM) — the vitest config
// excludes node:test files by derivation, and `npm run test:node` runs this.

test('TEST-PATHSUB-1: undefined capture throws Missing required parameter', () => {
  assert.throws(
    () => substitutePathParams('/api/series/{slug}', { slug: undefined }),
    /^Error: Missing required parameter: slug$/,
  )
})

test('TEST-PATHSUB-2: null capture throws Missing required parameter (the #502 semantic contract)', () => {
  // The #502 contract: a null capture is a programming error, never the
  // literal segment "null". This leg pins the helper's semantics; the
  // transport-level leg (TEST-PATHSUB-7) is the one that goes RED against the
  // old `!== undefined`-only guard in core.ts::performCall.
  assert.throws(
    () => substitutePathParams('/api/series/{slug}', { slug: null }),
    /^Error: Missing required parameter: slug$/,
  )
})

test('TEST-PATHSUB-3: empty string substitutes the empty segment (current behaviour kept)', () => {
  const { path, captures } = substitutePathParams('/api/series/{slug}', {
    slug: '',
  })
  assert.equal(path, '/api/series/')
  assert.deepEqual(captures, ['slug'])
})

test('TEST-PATHSUB-4: numbers are stringified', () => {
  const { path } = substitutePathParams('/api/series/{slug}', { slug: 42 })
  assert.equal(path, '/api/series/42')
})

test('TEST-PATHSUB-5: multiple captures substitute in template order and report names', () => {
  const { path, captures } = substitutePathParams(
    '/api/series/{slug}/chapters/{id}',
    { slug: 'abc', id: 7 },
  )
  assert.equal(path, '/api/series/abc/chapters/7')
  assert.deepEqual(captures, ['slug', 'id'])
})

test('TEST-PATHSUB-6: captured names leak into neither the template nor the GET query (transport)', async () => {
  setBaseUrlResolver(() => Promise.resolve('http://test'))
  const seen: string[] = []
  const restoreFetch = stubFetch(async (url: string) => {
    seen.push(url)
    return new Response('{"ok":true}', {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  })
  try {
    const result = await callAsync(
      'GET /api/series/{slug}',
      { slug: 'abc', q: 'x' },
      { noCoalesce: true },
    )
    assert.deepEqual(result, { ok: true })
    assert.equal(seen.length, 1)
    assert.equal(
      seen[0],
      'http://test/api/series/abc?q=x',
      'the captured param must be substituted in the path and must NOT appear again in the query string',
    )
  } finally {
    restoreFetch()
  }
})

test('TEST-PATHSUB-7: the browser transport throws for a null capture, not a "null" segment (#502 — RED before the fix)', async () => {
  setBaseUrlResolver(() => Promise.resolve('http://test'))
  const restoreFetch = stubFetch(async () => {
    throw new Error('fetch must not be called for a null capture')
  })
  try {
    await assert.rejects(
      callAsync('GET /api/series/{slug}', { slug: null }, { noCoalesce: true }),
      /^Error: Missing required parameter: slug$/,
    )
  } finally {
    restoreFetch()
  }
})

test('TEST-PATHSUB-8: FormData branch — absent capture throws, present value substitutes (semantics kept)', async () => {
  setBaseUrlResolver(() => Promise.resolve('http://test'))
  // FormData is a Node global since v18; the SDK guard (`isFormData`) already
  // falls back to the object branch where it is undefined (SSR/node).
  const missing = new FormData()
  await assert.rejects(
    callAsync('POST /api/uploads/{id}/files', missing, { noCoalesce: true }),
    /^Error: Missing required parameter: id$/,
  )

  const seen: string[] = []
  const restoreFetch = stubFetch(async (url: string) => {
    seen.push(url)
    return new Response('{"ok":true}', {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  })
  try {
    const present = new FormData()
    present.set('id', 'f-1')
    await callAsync('POST /api/uploads/{id}/files', present, {
      noCoalesce: true,
    })
    assert.equal(seen[0], 'http://test/api/uploads/f-1/files')
  } finally {
    restoreFetch()
  }
})

/** Swap `globalThis.fetch` for the test double; returns the restore fn. */
function stubFetch(
  impl: (url: string, init?: RequestInit) => Promise<Response>,
): () => void {
  const prev = globalThis.fetch
  ;(globalThis as any).fetch = impl
  return () => {
    ;(globalThis as any).fetch = prev
  }
}
