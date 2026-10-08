import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createLazyActionSequencer, createLazyDispatcher } from './lazy-dispatch.ts'

/**
 * #659 — cross-action dispatch order on ONE store.
 *
 * `createLazyDispatcher` (invocation order: `resolveImpl().then(impl =>
 * impl(...args))`) runs each action's body in CHUNK-LOAD order, not CALL order,
 * because every action has its own chunk and its own resolver. Two actions
 * called back-to-back on the same store (`X.edit(patch); X.save()`) therefore
 * race: with cold chunks, `save` can run before `edit` and write the PREVIOUS
 * draft. This was a real production bug (kid-mode switch saved ON after being
 * clicked OFF).
 *
 * The class fix: ONE call-order sequencer per store, shared by all of that
 * store's lazy dispatchers (`LazyDispatchOptions.sequencer`). Chain ordering
 * is by START, not SETTLE — a slow action must not serialize later ones behind
 * its network call, and an action that dispatches + awaits another action on
 * its own store must not deadlock.
 *
 * TEST-11 is the load-bearing ordering spec (RED at d62f2c4 with
 * chunk-controlled resolution); TEST-12/13 prove a failed dispatch releases
 * the chain; TEST-14 is the re-entrancy deadlock guard; TEST-15 proves the
 * chain is per-store; TEST-16 covers preload's non-blocking guarantee.
 */

/** Flush all pending microtasks + macrotasks (deterministic under node).
 *  One `setTimeout(0)` macrotask turn flushes every queued microtask, and the
 *  controlled promises below never need more than one turn. */
const settleTicks = () => new Promise<void>(r => setTimeout(r, 0))

test('TEST-11: two lazy actions on one store run in CALL order, not chunk-load order', { timeout: 5000 }, async () => {
  const calls: string[] = []
  // ONE sequencer per store — the store-kit wiring (`store-kit.ts makeBuilder`).
  const sequencer = createLazyActionSequencer()

  let releaseA!: () => void
  const chunkA = new Promise<void>(r => (releaseA = r))

  // `a`'s chunk resolves DELIBERATELY LATER than `b`'s (the cold-chunk race):
  // without sequencing, `b`'s impl runs first and the store sees write order
  // b-then-a even though the handler called a(); b().
  const a = createLazyDispatcher(
    async () => {
      await chunkA
      return { default: () => () => calls.push('a') }
    },
    (m: any) => m.default(),
    { sequencer },
  )
  const b = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('b') }),
    (m: any) => m.default(),
    { sequencer },
  )

  const pa = a()
  const pb = b()

  // `b`'s chunk has LONG resolved by now, but `b`'s impl must NOT have been
  // invoked: `a` was dispatched first and its impl has not started yet.
  await settleTicks()
  assert.deepEqual(calls, [] as string[], 'b must wait for a to START, even though b\'s chunk resolved first')

  releaseA()
  await Promise.all([pa, pb])
  assert.deepEqual(calls, ['a', 'b'], 'impl start order must match CALL order')
})

test('TEST-12: an import failure in action a does not block action b on the same store', { timeout: 5000 }, async () => {
  const calls: string[] = []
  const sequencer = createLazyActionSequencer()
  const a = createLazyDispatcher(
    async () => {
      throw new Error('a chunk gone')
    },
    (m: any) => m.default(),
    // Zero the retry backoff: the failure-budget assertions are about ATTEMPTS,
    // not wall clock (the injectable seam lazy-dispatch's failure specs exist for).
    { sequencer, sleep: () => Promise.resolve() },
  )
  const b = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('b') }),
    (m: any) => m.default(),
    { sequencer },
  )

  const pa = a()
  const pb = b()

  await assert.rejects(pa, /a chunk gone/, 'the failed dispatch surfaces its rejection')
  await pb
  assert.deepEqual(calls, ['b'], 'b must still run after a\'s import failure')
})

test('TEST-13: a synchronous throw in a\'s impl does not block b on the same store', { timeout: 5000 }, async () => {
  const calls: string[] = []
  const sequencer = createLazyActionSequencer()
  const a = createLazyDispatcher(
    async () => ({
      default: () => () => {
        throw new Error('impl boom')
      },
    }),
    (m: any) => m.default(),
    { sequencer },
  )
  const b = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('b') }),
    (m: any) => m.default(),
    { sequencer },
  )

  const pa = a()
  const pb = b()

  await assert.rejects(pa, /impl boom/)
  await pb
  assert.deepEqual(calls, ['b'], 'b must still run after a\'s impl threw synchronously')
})

test('TEST-14: re-entrancy — a dispatches b (same store) and awaits it, without deadlock', { timeout: 5000 }, async () => {
  const calls: string[] = []
  const sequencer = createLazyActionSequencer()

  const b = createLazyDispatcher(
    async () => ({ default: () => () => 'b-ok' }),
    (m: any) => m.default(),
    { sequencer },
  )
  const a = createLazyDispatcher(
    async () => ({
      default: () => async () => {
        // Dispatch + AWAIT another action on the SAME store from inside this
        // action's body. If the sequencer waited for SETTLE, this would
        // deadlock (a waits for b, b waits for a's turn); it must wait for b's
        // START only.
        calls.push('a-before')
        const r = await b()
        calls.push('a-after:' + r)
      },
    }),
    (m: any) => m.default(),
    { sequencer },
  )

  await a()
  assert.deepEqual(calls, ['a-before', 'a-after:b-ok'])
})

test('TEST-15: different stores are not sequenced against each other', { timeout: 5000 }, async () => {
  const calls: string[] = []
  const sequencerX = createLazyActionSequencer()
  const sequencerY = createLazyActionSequencer()

  let releaseX!: () => void
  const chunkX = new Promise<void>(r => (releaseX = r))
  const x = createLazyDispatcher(
    async () => {
      await chunkX
      return { default: () => () => calls.push('x') }
    },
    (m: any) => m.default(),
    { sequencer: sequencerX },
  )
  const y = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('y') }),
    (m: any) => m.default(),
    { sequencer: sequencerY },
  )

  const px = x()
  const py = y()

  // Store Y's action runs even though store X's chunk is still pending —
  // sequencing is per-store, never global.
  await settleTicks()
  assert.deepEqual(calls, ['y'], 'store Y must not be delayed by store X\'s slow chunk')
  releaseX()
  await Promise.all([px, py])
  assert.deepEqual(calls, ['y', 'x'])
})

test('TEST-16: preload() stays non-blocking and unsequenced (never invokes, never holds the chain)', { timeout: 5000 }, async () => {
  const calls: string[] = []
  const sequencer = createLazyActionSequencer()

  const a = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('a') }),
    (m: any) => m.default(),
    { sequencer },
  )

  // Warm a chunk without invoking: `preload()` resolves and the impl never ran.
  await a.preload()
  assert.deepEqual(calls, [] as string[], 'preload must never invoke the impl')

  // A dispatch in flight must not block ANOTHER action's preload on the same
  // store: `slow` claims the chain and parks on its still-pending chunk;
  // `fast.preload()` must resolve anyway (preload does not take a chain slot).
  let releaseSlow!: () => void
  const slowChunk = new Promise<void>(r => (releaseSlow = r))
  const slow = createLazyDispatcher(
    async () => {
      await slowChunk
      return { default: () => () => calls.push('slow') }
    },
    (m: any) => m.default(),
    { sequencer },
  )
  const fast = createLazyDispatcher(
    async () => ({ default: () => () => calls.push('fast') }),
    (m: any) => m.default(),
    { sequencer },
  )

  const p = slow()
  await fast.preload()
  assert.deepEqual(calls, [] as string[], 'preload must not wait for (or occupy) the chain')
  releaseSlow()
  await p
  assert.deepEqual(calls, ['slow'])
})
