import { test } from 'node:test'
import assert from 'node:assert/strict'
import {
  getAuthToken,
  setAuthToken,
  setAuthTokenProvider,
} from './core.ts'

// FE-2 smoke — the injectable auth-token provider. Proves the fix to the
// silent-unauthenticated foot-gun: an app that doesn't use a zustand-persist
// store named `auth-storage` can inject its token and requests authenticate,
// while the DEFAULT (no provider) still reads `localStorage['auth-storage']`
// so ziee is unaffected.

// Minimal localStorage stub for the default path.
function stubLocalStorage(store: Record<string, string>) {
  ;(globalThis as any).localStorage = {
    getItem: (k: string) => (k in store ? store[k] : null),
    setItem: (k: string, v: string) => {
      store[k] = v
    },
    removeItem: (k: string) => {
      delete store[k]
    },
  }
}

test('TEST-FE2-1: default reads localStorage["auth-storage"] {state:{token}}', () => {
  setAuthTokenProvider(null) // ensure default path
  stubLocalStorage({
    'auth-storage': JSON.stringify({ state: { token: 'ls-token' } }),
  })
  assert.equal(getAuthToken(), 'ls-token')
})

test('TEST-FE2-2: default returns null when auth-storage absent/corrupt', () => {
  setAuthTokenProvider(null)
  stubLocalStorage({})
  assert.equal(getAuthToken(), null)
  stubLocalStorage({ 'auth-storage': 'not-json{' })
  assert.equal(getAuthToken(), null)
})

test('TEST-FE2-3: setAuthTokenProvider overrides the default source', () => {
  stubLocalStorage({
    'auth-storage': JSON.stringify({ state: { token: 'ls-token' } }),
  })
  setAuthTokenProvider(() => 'injected-token')
  assert.equal(getAuthToken(), 'injected-token')
  setAuthTokenProvider(null) // restore default
  assert.equal(getAuthToken(), 'ls-token')
})

test('TEST-FE2-4: setAuthToken sets a static token and null clears it', () => {
  stubLocalStorage({}) // no localStorage token
  setAuthToken('static-token')
  assert.equal(getAuthToken(), 'static-token')
  setAuthToken(null)
  assert.equal(getAuthToken(), null)
  setAuthTokenProvider(null) // restore default for other tests
})
