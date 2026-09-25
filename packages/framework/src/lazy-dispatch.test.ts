import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createLazyDispatcher } from './lazy-dispatch.ts'

// issue #161 / FUP-65 (ITEM-5b of queue 272's plan): an on-demand click made
// offline used to stay broken after reconnect until a full page reload — every
// retry re-`import()`ed the SAME bundler-generated specifier, which a browser's
// module map caches as permanently failed once it has failed once (per the
// HTML spec — see lazy-dispatch.ts's header).
//
// The fix does NOT skip the first, real attempt while offline (a deliberate
// offline probe — e.g. the app's own offline-chunk-recovery e2e spec's
// negative control — must still see the browser's own native
// "Failed to fetch dynamically imported module: <url>" failure). Instead,
// once that first attempt has failed and told us its URL, every LATER
// attempt — this call's own remaining retries, or a wholly separate later
// dispatch — re-fetches a CACHE-BUSTED variant of that url through the
// injectable `importUrl` seam (a real dynamic import is not testable under
// plain Node against a synthetic URL), never the original loader again.
// `navigator.onLine` is driven directly (`isOffline()` only reads that one
// property).

/**
 * Swap `navigator` for a fake with a controllable `onLine`, restored after.
 *
 * ALWAYS async and ALWAYS awaits `run()` before restoring: `run` is typically
 * an async function whose body keeps executing (via internal `await`s) well
 * past the point a plain `try { return run() } finally { restore }` would
 * already have restored the ORIGINAL navigator — a synchronous `finally` runs
 * before a returned-but-unawaited promise settles, which silently un-does the
 * offline simulation mid-dispatch (caught here directly: it manifested as
 * extra retry-loop iterations the "never loop while offline" gate should have
 * skipped).
 */
async function withNavigator<T>(onLine: boolean, run: () => Promise<T>): Promise<T> {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator')
  Object.defineProperty(globalThis, 'navigator', {
    value: { onLine },
    configurable: true,
  })
  try {
    return await run()
  } finally {
    if (original) Object.defineProperty(globalThis, 'navigator', original)
    else delete (globalThis as { navigator?: unknown }).navigator
  }
}

const FAILURE_URL = 'https://app.example/assets/toggleSidebar-abc123.js'
const nativeImportFailure = () =>
  new Error(`Failed to fetch dynamically imported module: ${FAILURE_URL}`)

test('OFFLINE-A: the FIRST attempt while offline still calls the real loader (a deliberate offline probe must see the native failure), and does not loop', async () => {
  await withNavigator(false, async () => {
    let loaderCalls = 0
    let sleeps = 0
    const dispatcher = createLazyDispatcher(
      () => {
        loaderCalls++
        return Promise.reject(nativeImportFailure())
      },
      (mod: { default: unknown }) => mod.default as (...args: unknown[]) => unknown,
      { sleep: async () => void sleeps++ },
    )

    await assert.rejects(() => dispatcher())

    // The first-ever attempt DID happen — this is what a negative control
    // (e.g. the app's Leg A) observes as the browser's native failure line.
    assert.equal(loaderCalls, 1)
    // "Never loop while offline": once that attempt failed and gave us a URL
    // to work with, no further backoff-and-retry was spent chasing a doomed
    // request (original OR cache-busted) while still offline.
    assert.equal(sleeps, 0)
  })
})

test('OFFLINE-B: a dispatch that failed while offline recovers on the NEXT call once online, via a cache-busted re-import (never the original loader again)', async () => {
  let loaderCalls = 0
  const importedUrls: string[] = []
  const dispatcher = createLazyDispatcher(
    () => {
      loaderCalls++
      return Promise.reject(nativeImportFailure())
    },
    (mod: { default: unknown }) => mod.default as (...args: unknown[]) => unknown,
    {
      importUrl: async (url: string) => {
        importedUrls.push(url)
        return { default: () => 'ok' }
      },
    },
  )

  await withNavigator(false, async () => {
    await assert.rejects(() => dispatcher())
  })
  assert.equal(loaderCalls, 1, 'the first, real attempt happened while offline')
  assert.equal(importedUrls.length, 0, 'no cache-busted retry was attempted while still offline')

  // Reconnect. A LATER dispatch (a new click) must succeed — not repeat the
  // earlier failure, which is exactly what #161 reported ("stays broken after
  // reconnect until reload").
  await withNavigator(true, async () => {
    const result = await dispatcher()
    assert.equal(result, 'ok')
  })

  // The recovery used a CACHE-BUSTED variant of the failed URL (never the
  // original loader again — that specifier is permanently poisoned).
  assert.equal(loaderCalls, 1, 'the original loader is never called again once a URL is known')
  assert.equal(importedUrls.length, 1)
  assert.match(importedUrls[0], /^https:\/\/app\.example\/assets\/toggleSidebar-abc123\.js\?ziee_retry=\d+$/)
})

test('ONLINE regression guard: normal transient-failure retry-then-succeed (no URL in the error) is untouched', async () => {
  let calls = 0
  const dispatcher = createLazyDispatcher(
    () => {
      calls++
      if (calls === 1) return Promise.reject(new Error('transient blip'))
      return Promise.resolve({ default: () => 'ok' })
    },
    (mod: { default: unknown }) => mod.default as (...args: unknown[]) => unknown,
    { sleep: async () => {} },
  )

  await withNavigator(true, async () => {
    const result = await dispatcher()
    assert.equal(result, 'ok')
  })
  // The dispatcher's own bounded in-call retry recovered within one dispatch,
  // via the ORIGINAL loader (the error carried no URL to cache-bust) — no
  // online/offline gating involved when the network is up throughout.
  assert.equal(calls, 2)
})
