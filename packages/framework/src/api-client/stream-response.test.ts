import { test } from 'node:test'
import assert from 'node:assert/strict'
import { callAsync, setAuthTokenProvider, setBaseUrlResolver } from './core.ts'
import { __isolateForTests, __resetInflightForTests, inflightSize } from './inflight.ts'

// ── `responseType: 'stream'` + the `isolate` hard-fail ──────────────────────────────
//
// Two changes to a transport shared by three applications, so each is asserted for what
// it does AND for what it must not disturb.
//
// The bar these have to clear is not "a stream comes back". It is that the raw-body path
// cannot be reached by accident (every existing call keeps the parse switch), cannot be
// joined (one body has one reader), and that the non-cloneable case fails where it is
// caused rather than three frames later with an unrelated `TypeError: locked`.

function stubFetch(impl: (url: string, init?: any) => Promise<Response>): () => void {
  const prev = globalThis.fetch
  ;(globalThis as any).fetch = impl
  return () => {
    ;(globalThis as any).fetch = prev
  }
}

/** A 200 whose body arrives in two pieces, so "resolved at HEADERS" is observable. */
function twoChunkResponse(a: string, b: string): Response {
  let release!: () => void
  const gate = new Promise<void>(r => {
    release = r
  })
  const body = new ReadableStream<Uint8Array>({
    async start(controller) {
      controller.enqueue(new TextEncoder().encode(a))
      await gate
      controller.enqueue(new TextEncoder().encode(b))
      controller.close()
    },
  })
  const res = new Response(body, {
    status: 200,
    headers: { 'Content-Type': 'application/octet-stream' },
  })
  ;(res as any).__release = release
  return res
}

function setup(): void {
  setAuthTokenProvider(() => 'test-token')
  setBaseUrlResolver(async () => 'http://stub.invalid')
  __resetInflightForTests()
}

test('SDKSTREAM-1: responseType:"stream" yields the RAW body, resolved at headers', async () => {
  setup()
  const res = twoChunkResponse('first-', 'second')
  const restore = stubFetch(async () => res)
  try {
    const stream = (await callAsync<ReadableStream<Uint8Array>>(
      'GET /api/x',
      {},
      { responseType: 'stream' },
    )) as ReadableStream<Uint8Array>

    // It is the raw body, not a Blob and not parsed text.
    assert.ok(stream instanceof ReadableStream, 'a stream body is returned unparsed')

    // Resolved at HEADERS: the first chunk is readable while the second is still gated.
    const reader = stream.getReader()
    const first = await reader.read()
    assert.equal(new TextDecoder().decode(first.value), 'first-')
    ;(res as any).__release()
    const second = await reader.read()
    assert.equal(new TextDecoder().decode(second.value), 'second')
    assert.equal((await reader.read()).done, true)
  } finally {
    restore()
  }
})

test('SDKSTREAM-2: omitting responseType is UNCHANGED — the parse switch still runs', async () => {
  // The additivity claim, asserted rather than argued. Every existing caller omits the
  // option, so this is the leg that says the change reaches none of them.
  setup()
  const restore = stubFetch(
    async () =>
      new Response(JSON.stringify({ ok: 1 }), {
        status: 200,
        headers: { 'Content-Type': 'application/json' },
      }),
  )
  try {
    const out = await callAsync<{ ok: number }>('GET /api/y', {})
    assert.deepEqual(out, { ok: 1 }, 'a JSON call still parses to an object')
  } finally {
    restore()
  }
})

test('SDKSTREAM-3: a stream call is NEVER joined — two callers get two bodies', async () => {
  // The hazard ITEM-17b names. Under coalescing both callers would receive ONE body;
  // the first `getReader()` locks it and the second throws `TypeError: locked` far from
  // the cause. Excluding it in the `joinable` predicate is what makes that impossible.
  setup()
  let calls = 0
  const restore = stubFetch(async () => {
    calls++
    return new Response(new TextEncoder().encode('abc'), {
      status: 200,
      headers: { 'Content-Type': 'application/octet-stream' },
    })
  })
  try {
    const [a, b] = await Promise.all([
      callAsync<ReadableStream<Uint8Array>>('GET /api/z', {}, { responseType: 'stream' }),
      callAsync<ReadableStream<Uint8Array>>('GET /api/z', {}, { responseType: 'stream' }),
    ])
    assert.equal(calls, 2, 'two stream callers issue two requests — never one shared body')
    assert.notEqual(a, b, 'and receive two DISTINCT streams')
    // Both are independently readable — the property the sharing bug destroys.
    await (a as ReadableStream<Uint8Array>).getReader().read()
    await (b as ReadableStream<Uint8Array>).getReader().read()
  } finally {
    restore()
  }
})

test('SDKSTREAM-4: plain GETs still coalesce — the exclusion is narrow', async () => {
  // The regression control for SDKSTREAM-3: if the new predicate term accidentally
  // disabled coalescing generally, this goes red and the 2,749→1 class of win the
  // transport provides elsewhere would have been silently traded away.
  setup()
  let calls = 0
  const restore = stubFetch(async () => {
    calls++
    await new Promise(r => setTimeout(r, 5))
    return new Response(JSON.stringify({ n: calls }), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    })
  })
  try {
    const [a, b] = await Promise.all([
      callAsync<{ n: number }>('GET /api/joinme', {}),
      callAsync<{ n: number }>('GET /api/joinme', {}),
    ])
    assert.equal(calls, 1, 'two identical plain GETs still collapse into ONE request')
    assert.deepEqual(a, b)
    assert.notEqual(a, b as unknown, 'and the joiner gets an ISOLATED copy, not the same object')
  } finally {
    restore()
    __resetInflightForTests()
    assert.equal(inflightSize(), 0)
  }
})

test('SDKSTREAM-5: a non-cloneable joined value THROWS where it is caused', () => {
  // The `isolate` hard-fail, driven DIRECTLY through the test seam.
  //
  // It is unreachable through the public paths today — the parse switch yields JSON /
  // string / Blob, all cloneable or primitive, and a stream call cannot join — so calling
  // it is the only honest way to assert it. Asserting it by inference ("streams don't
  // join, therefore the guard holds") would test the exclusion twice and the guard never.
  //
  // Before this change the same input was SHARED, and the failure surfaced later and
  // elsewhere as `TypeError: locked` on a body someone else had already read.
  assert.throws(
    () => __isolateForTests(new ReadableStream()),
    /could not be isolated/,
    'a non-cloneable joined value must throw, not be silently shared',
  )
  // …and the cloneable cases it must NOT disturb — the regression control, because a
  // guard that threw on everything would also pass the assertion above.
  assert.deepEqual(__isolateForTests({ a: [1, 2] }), { a: [1, 2] })
  assert.equal(__isolateForTests('plain'), 'plain')
  assert.equal(__isolateForTests(null), null)
})
