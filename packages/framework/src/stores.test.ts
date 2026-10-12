import { test } from 'node:test'
import assert from 'node:assert/strict'
import { create } from 'zustand'
import { createStoreProxy } from './stores.ts'

// These specs load the REAL createStoreProxy under `node --test` (via the
// scripts/node-test-loader.mjs alias loader — proves ITEM-10). React + zustand
// are real; only the unrelated @/core/{module-system,events} boundaries the
// factory never calls are stubbed. Reading a reactive state VALUE outside a
// React render calls a hook with a null dispatcher and throws — which is
// exactly how we assert the render-only contract.

const mkProxy = (state: any) => createStoreProxy(create<any>(() => state) as any) as any

// `DEFAULT_DESTROY_DELAY_MS` in `stores.ts` is not exported; pin the default
// here so this suite moves with it (TEST-11/TEST-12 guard its destroy timer).
const DEFAULT_DESTROY_DELAY_MS = 5000

test('TEST-1: an action is callable hook-free OUTSIDE any render and mutates state', () => {
  const store = create<any>(set => ({
    count: 0,
    inc: () => set((s: any) => ({ count: s.count + 1 })),
  }))
  const p: any = createStoreProxy(store as any)
  // Called at module/test scope — NOT inside a React render. Must not throw.
  assert.doesNotThrow(() => p.inc())
  p.inc()
  assert.equal(store.getState().count, 2)
  assert.equal(p.$.count, 2)
})

test('TEST-2: `$` returns the getState() snapshot hook-free outside render', () => {
  const store = create<any>(() => ({ a: 1, b: 'x' }))
  const p: any = createStoreProxy(store as any)
  assert.doesNotThrow(() => p.$)
  assert.equal(p.$.a, store.getState().a)
  assert.equal(p.$.b, 'x')
  assert.deepEqual(p.$, store.getState())
})

test('TEST-3: reading a reactive state VALUE outside render throws (render-only contract)', () => {
  const p = mkProxy({ value: 42 })
  // A non-function, non-special prop routes through useEffect/useStore → hook
  // call with no dispatcher → throws. `$` is the required handler-side escape.
  assert.throws(() => p.value)
})

test('TEST-4/TEST-10: `.__state` is no longer a hook-free special; only `$` is', () => {
  const p = mkProxy({ value: 42 })
  // `$` is a valid hook-free snapshot escape…
  assert.doesNotThrow(() => p.$)
  assert.equal(typeof p.$, 'object')
  // …but `.__state` lost its special status: it now behaves like any reactive
  // read and throws outside render (proving the alias was REMOVED, not renamed).
  // Bracket access hits the same proxy trap while sidestepping the grit ban that
  // (correctly) forbids the `.__state` member syntax in source.
  assert.throws(() => p['__state'])
  // Sibling internals are unaffected (not swept, not banned).
  assert.doesNotThrow(() => p.__setState)
  assert.equal(typeof p.__setState, 'function')
})

test('TEST-11: executeDestroy cancels its OWN pending destroy timer — __destroy__ runs exactly once', t => {
  let destroyRuns = 0
  const store = create<any>(() => ({
    n: 0,
    __destroy__: () => {
      destroyRuns++
    },
  }))
  const p: any = createStoreProxy(store as any)
  const rt = p.__refTracker

  t.mock.timers.enable({ apis: ['setTimeout'] })

  // Arm the 5s destroy timer the real way: dropping the last ref.
  rt.addRef('n')
  assert.equal(rt.destroyTimeoutId, null)
  rt.removeRef('n')
  assert.notEqual(rt.destroyTimeoutId, null)

  // Direct teardown while the timer is pending (module teardown / tests).
  // executeDestroy must clear the pending timer itself AND run the destroy
  // hook once, now — the stale timeout must not destroy the store again.
  rt.executeDestroy()
  assert.equal(destroyRuns, 1)
  assert.equal(rt.destroyed, true)
  assert.equal(rt.destroyTimeoutId, null)

  // The timer was cancelled: advancing past the delay runs nothing more.
  t.mock.timers.tick(DEFAULT_DESTROY_DELAY_MS + 1000)
  assert.equal(destroyRuns, 1)
})

test('TEST-12: executeDestroy is idempotent against a destroy already run', t => {
  let destroyRuns = 0
  let initRuns = 0
  const store = create<any>(() => ({
    n: 0,
    __destroy__: () => {
      destroyRuns++
    },
    // The proxy's init latch: runs in the get trap on `storeInitialized === false`.
    __init__: {
      __store__: () => {
        initRuns++
      },
    },
  }))
  const p: any = createStoreProxy(store as any)
  const rt = p.__refTracker

  t.mock.timers.enable({ apis: ['setTimeout'] })

  // Let the scheduled timer fire first — the destroy already ran once.
  rt.scheduleDestroy()
  t.mock.timers.tick(DEFAULT_DESTROY_DELAY_MS)
  assert.equal(destroyRuns, 1)
  assert.equal(rt.destroyed, true)

  // A LIVE lifetime: one read runs `init` (and then throws the render-only
  // hook error, which is how the read is counted without a component).
  assert.throws(() => p.n)
  assert.equal(initRuns, 1)

  // A second, DIRECT teardown after that (module teardown racing the timer)
  // must not run the destroy hook again…
  rt.executeDestroy()
  assert.equal(destroyRuns, 1)

  // …but it MUST still re-arm the init latch — a store a page comes back to
  // re-runs `init` on the next read, on the FIRST access, before any
  // `addRef`/`reset` can fire.
  assert.throws(() => p.n)
  assert.equal(initRuns, 2)
})
