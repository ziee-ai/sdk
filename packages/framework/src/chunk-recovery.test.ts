import { test } from 'node:test'
import assert from 'node:assert/strict'
import { isOffline, warmUnlessOffline, type OnlineGateEnv } from './chunk-recovery.ts'

// issue #161 / FUP-65 (ITEM-5b): `warmUnlessOffline` is the shared gate
// `store-kit.ts`'s `autoWarmLazyActions` and the shell's `usePrefetchModules`
// both wrap their warm-up scheduling in, so a warm never calls the underlying
// import() while offline (which would permanently poison the chunk — see
// lazy-dispatch.ts) and instead resumes on the next `online` event. These
// specs use the injected `OnlineGateEnv` seam (same pattern as
// `LazyDispatchOptions.sleep`), so no real browser globals are needed here.

test('GATE-1: warmUnlessOffline runs the warm immediately when online', () => {
  let calls = 0
  const env: OnlineGateEnv = { isOffline: () => false, onOnline: () => () => {} }
  const cleanup = warmUnlessOffline(() => {
    calls++
  }, env)
  assert.equal(calls, 1)
  assert.doesNotThrow(() => cleanup())
})

test('GATE-2: warmUnlessOffline defers to the next `online` event when offline, and runs exactly once', () => {
  let calls = 0
  let captured: (() => void) | null = null
  let unsubscribed = false
  const env: OnlineGateEnv = {
    isOffline: () => true,
    onOnline: cb => {
      captured = cb
      return () => {
        unsubscribed = true
      }
    },
  }
  const cleanup = warmUnlessOffline(() => {
    calls++
  }, env)

  assert.equal(calls, 0, 'must not warm while offline')
  assert.equal(typeof captured, 'function')

  const resumeWarm = captured as unknown as () => void
  resumeWarm()
  assert.equal(calls, 1)

  cleanup()
  assert.equal(unsubscribed, true)
})

test('GATE-3: isOffline() reads navigator.onLine, and reads "online" off-browser (no navigator global)', () => {
  const original = Object.getOwnPropertyDescriptor(globalThis, 'navigator')
  try {
    Object.defineProperty(globalThis, 'navigator', {
      value: { onLine: false },
      configurable: true,
    })
    assert.equal(isOffline(), true)

    Object.defineProperty(globalThis, 'navigator', {
      value: { onLine: true },
      configurable: true,
    })
    assert.equal(isOffline(), false)

    delete (globalThis as { navigator?: unknown }).navigator
    assert.equal(isOffline(), false, 'no navigator (SSR / node) reads as online')
  } finally {
    if (original) Object.defineProperty(globalThis, 'navigator', original)
    else delete (globalThis as { navigator?: unknown }).navigator
  }
})
