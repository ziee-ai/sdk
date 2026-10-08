import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  callAsync,
  setAuthTokenProvider,
  setBaseUrlResolver,
  setUnauthorizedHandler,
} from './core.ts'
import { __resetInflightForTests } from './inflight.ts'

// ── silent refresh vs. a wrong CURRENT password ────────────────────────────────────────
//
// The recovery management routes (and a username change) answer a wrong current password
// with 401 INVALID_CREDENTIALS, and an expired access token with a different 401. Silently
// refreshing and retrying the first would submit the wrong password twice and spend two of
// the account's few re-authentication attempts on one mistake; skipping the refresh for the
// second would fail an ordinary edit on an expired token. So the exemption is by the 401's
// error_code, and these cases pin BOTH halves (and the always-exempt change-password route).

function stubFetch(impl: (url: string, init?: any) => Promise<Response>): () => void {
  const prev = globalThis.fetch
  ;(globalThis as any).fetch = impl
  return () => {
    ;(globalThis as any).fetch = prev
  }
}

const unauthorized = (error_code: string) =>
  new Response(JSON.stringify({ error: 'x', error_code }), {
    status: 401,
    headers: { 'Content-Type': 'application/json' },
  })

function setup(refresh: () => Promise<boolean>) {
  setAuthTokenProvider(() => 'test-token')
  setBaseUrlResolver(async () => 'http://stub.invalid')
  setUnauthorizedHandler(refresh)
  __resetInflightForTests()
}

test('REFRESH-1: a wrong current password on a re-auth route is NOT retried after a refresh', async () => {
  let refreshes = 0
  let fetches = 0
  setup(async () => {
    refreshes += 1
    return true
  })
  const restore = stubFetch(async () => {
    fetches += 1
    return unauthorized('INVALID_CREDENTIALS')
  })
  try {
    for (const endpoint of [
      'POST /api/auth/recovery/codes',
      'POST /api/auth/recovery/codes/clear',
      'PUT /api/auth/recovery/questions',
      'POST /api/auth/recovery/questions/clear',
      'POST /api/auth/profile',
    ]) {
      __resetInflightForTests()
      await assert.rejects(callAsync(endpoint, { current_password: 'x' }))
    }
    assert.equal(refreshes, 0, 'no silent refresh for a credential refusal')
    assert.equal(fetches, 5, 'each request was sent exactly once')
  } finally {
    restore()
    setUnauthorizedHandler(null)
  }
})

test('REFRESH-2: an EXPIRED token on the same routes still refreshes and retries once', async () => {
  let refreshes = 0
  let fetches = 0
  setup(async () => {
    refreshes += 1
    return true
  })
  const restore = stubFetch(async () => {
    fetches += 1
    return fetches === 1
      ? unauthorized('TOKEN_EXPIRED')
      : new Response('{}', { status: 200, headers: { 'Content-Type': 'application/json' } })
  })
  try {
    await callAsync('POST /api/auth/profile', { display_name: 'Nice Name' })
    assert.equal(refreshes, 1, 'the expired token was refreshed')
    assert.equal(fetches, 2, 'and the request retried once with the fresh token')
  } finally {
    restore()
    setUnauthorizedHandler(null)
  }
})

test('REFRESH-3: change-password stays exempt whatever the 401 says', async () => {
  let refreshes = 0
  setup(async () => {
    refreshes += 1
    return true
  })
  const restore = stubFetch(async () => unauthorized('TOKEN_EXPIRED'))
  try {
    await assert.rejects(callAsync('POST /api/auth/password', { current_password: 'x', new_password: 'y' }))
    assert.equal(refreshes, 0)
  } finally {
    restore()
    setUnauthorizedHandler(null)
  }
})
