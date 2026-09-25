import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createLazyDispatcher } from './lazy-dispatch.ts'

// issue #161 / FUP-65 (ITEM-5b of queue 272's plan): an on-demand click made
// offline used to stay broken after reconnect until a full page reload,
// because `importModule()` was called unconditionally — including while
// offline — and a failed dynamic `import()` permanently poisons that
// specifier's module-map entry for the life of the document (see the HONEST
// LIMIT in lazy-dispatch.ts's header). These specs drive `navigator.onLine`
// directly (no DOM needed — `isOffline()` only reads that one property) and
// assert on whether the LOADER itself was ever called, which is the only
// observable proxy for "was the module-map entry poisoned".

/** Swap `navigator` for a fake with a controllable `onLine`, restored after. */
function withNavigator<T>(onLine: boolean, run: () => T): T {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator')
  Object.defineProperty(globalThis, 'navigator', {
    value: { onLine },
    configurable: true,
  })
  try {
    return run()
  } finally {
    if (original) Object.defineProperty(globalThis, 'navigator', original)
    else delete (globalThis as { navigator?: unknown }).navigator
  }
}

test('OFFLINE-A: a dispatch made while offline never calls the loader, and never spends the real backoff', async () => {
  await withNavigator(false, async () => {
    let loaderCalls = 0
    let sleeps = 0
    const dispatcher = createLazyDispatcher(
      () => {
        loaderCalls++
        return Promise.reject(new Error('must never be called while offline'))
      },
      (mod: { default: unknown }) => mod.default as (...args: unknown[]) => unknown,
      { sleep: async () => void sleeps++ },
    )

    await assert.rejects(() => dispatcher())

    // The load must never have been ATTEMPTED — attempting it is what poisons
    // the module map for the life of the document (the actual bug in #161).
    assert.equal(loaderCalls, 0)
    // "Never loop while offline": no backoff sleep spent chasing a doomed
    // attempt — fail fast instead.
    assert.equal(sleeps, 0)
  })
})

test('OFFLINE-B: a dispatch that failed while offline succeeds on the NEXT call once online — nothing was poisoned', async () => {
  let loaderCalls = 0
  const dispatcher = createLazyDispatcher(
    () => {
      loaderCalls++
      return Promise.resolve({ default: () => 'ok' })
    },
    (mod: { default: unknown }) => mod.default as (...args: unknown[]) => unknown,
  )

  await withNavigator(false, async () => {
    await assert.rejects(() => dispatcher())
  })
  // Still offline: the loader was never touched.
  assert.equal(loaderCalls, 0)

  // Reconnect. A LATER dispatch (a new click) must succeed outright — not
  // repeat the earlier failure, which is exactly what #161 reported ("stays
  // broken after reconnect until reload").
  await withNavigator(true, async () => {
    const result = await dispatcher()
    assert.equal(result, 'ok')
  })
  assert.equal(loaderCalls, 1)
})

test('ONLINE regression guard: normal transient-failure retry-then-succeed is untouched by the offline gate', async () => {
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
  // The dispatcher's own bounded in-call retry recovered within one dispatch —
  // no online/offline gating involved when the network is up throughout.
  assert.equal(calls, 2)
})
