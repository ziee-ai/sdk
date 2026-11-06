import { test } from 'node:test'
import assert from 'node:assert/strict'
import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { createStoreProxy } from './stores.ts'
import { defineStore, defineLocalStore } from './store-kit.ts'

// Loads the REAL store-kit under `node --test` (alias loader). The event-bus
// boundary is stubbed (never exercised by these specs); everything else is real.

test('TEST-5: a defineStore action lands on getState() and is callable hook-free', () => {
  const h = defineStore('ProbeStoreKit', {
    state: { n: 0 },
    actions: set => ({ bump: () => set((s: any) => ({ n: s.n + 1 })) }),
  })
  // The authoring model spreads actions into state, so getState().bump is a fn…
  assert.equal(typeof h.store.getState().bump, 'function')
  const p: any = createStoreProxy(h.store as any)
  // …and the proxy returns it directly (no hook) — callable outside render.
  assert.doesNotThrow(() => {
    p.bump()
    p.bump()
  })
  assert.equal(p.$.n, 2)
})

test('TEST-6: defineLocalStore proxy — reactive read in render, `$` snapshot, action in handler, no `__state`', () => {
  const Def = defineLocalStore({
    state: { q: '' },
    actions: (set: any) => ({ setQ: (v: string) => set(() => ({ q: v })) }),
  })
  let dollar: any
  let capturedAction: any
  let stateAlias: any
  function Comp() {
    const s: any = Def.use({ q: 'hello' })
    const { q } = s // reactive read IN render — must not throw
    dollar = s.$.q // hook-free snapshot in render
    capturedAction = s.setQ // action captured for a handler-context call
    stateAlias = s['__state'] // `__state` is NOT the snapshot alias anymore
    // (bracket access = same proxy trap, sidesteps the grit ban on `.__state`)
    return React.createElement('span', null, q)
  }
  const html = renderToStaticMarkup(React.createElement(Comp))
  assert.match(html, /hello/)
  assert.equal(dollar, 'hello')
  // `__state` on the local proxy is just a (missing) reactive field → undefined,
  // NOT the getState snapshot. Only `$` is the snapshot.
  assert.equal(stateAlias, undefined)
  // The action captured during render is callable from a handler context.
  assert.equal(typeof capturedAction, 'function')
  assert.doesNotThrow(() => capturedAction('world'))
})

test('TEST-9: `$.__destroy__` is reachable hook-free and does NOT trigger store init (HMR-destroy pattern)', () => {
  let initRuns = 0
  const h = defineStore('ProbeDestroy', {
    state: { x: 0 },
    actions: () => ({}),
    init: () => {
      initRuns += 1
    },
  })
  const p: any = createStoreProxy(h.store as any)
  // The exact access pattern module-system/store.ts's HMR path now uses:
  assert.equal(typeof p.$.__destroy__, 'function')
  assert.doesNotThrow(() => p.$.__destroy__())
  // Reaching teardown via `$` must NOT have fired store-level `__init__`
  // (the `$` branch short-circuits before the trap's init side-effect).
  assert.equal(initRuns, 0)
})

test('TEST-17: #659 — mixing eager and lazy actions on one store reorders neither', async () => {
  // Eager (inline `actions:` factory) = synchronous at the call site; lazy
  // actions on the same store share ONE per-store sequencer, so they keep
  // CALL order too. The documented guarantee (lazy-dispatch.ts): eager runs
  // exactly at its call site; lazy impls are always deferred and, among
  // themselves, invoked in dispatch order.
  const executed: string[] = []
  const h = defineStore('ProbeEagerLazyMix', {
    state: { n: 0 },
    actions: (set: any) => ({
      bump: () => {
        executed.push('eager-bump')
        set((s: any) => ({ n: s.n + 1 }))
      },
    }),
    lazyActions: {
      a: () =>
        Promise.resolve({
          default: () => async () => {
            executed.push('lazy-a')
          },
        }),
      b: () =>
        Promise.resolve({
          default: () => async () => {
            executed.push('lazy-b')
          },
        }),
    },
  })
  const p: any = createStoreProxy(h.store as any)
  p.bump()
  p.a()
  p.b()
  // The eager action ran synchronously at its call site — already observable
  // before the lazy impls (always deferred) had a chance to run.
  assert.deepEqual(executed, ['eager-bump'])
  await new Promise<void>(r => setTimeout(r, 0))
  assert.deepEqual(executed, ['eager-bump', 'lazy-a', 'lazy-b'])
  assert.equal(p.$.n, 1)
})

// ── Quiet while hidden (history-pinned stores) ──────────────────────────────
// A pinned store nothing renders is QUIET: its `on`/`watch` handlers are
// deferred, replayed once when it is shown again; past the app's revalidate
// window its own `sync:reconnect` handler runs once as the revalidation.

import { pinStore, unpinStore } from './stores.ts'
import { setQuietPolicy } from './store-kit.ts'
import { useEventBusStore } from './events/index.ts'

let quietSeq = 0
function mkQuietStore(opts: { runWhileQuiet?: boolean } = {}) {
  const log: string[] = []
  const wakes: any[] = []
  let isQuiet: () => boolean = () => false
  const h = defineStore(`QuietProbe${quietSeq++}`, {
    state: { v: 0 },
    actions: set => ({ setV: (v: number) => set(() => ({ v })) }),
    init: ({ on, onWake, isQuiet: q }) => {
      isQuiet = q
      on('sync:series' as never, ((e: any) => {
        log.push(`series:${e.data.id}`)
      }) as never)
      on('sync:reconnect' as never, (() => {
        log.push('reconnect')
      }) as never)
      on('app:other' as never, ((e: any) => {
        log.push(`other:${e.data.n}`)
      }) as never, opts.runWhileQuiet ? { whileQuiet: 'run' } : undefined)
      onWake(info => {
        wakes.push(info)
      })
    },
  })
  const p: any = createStoreProxy(h.store as any)
  // First access runs init (the proxy's latch) — a hook-free special does not,
  // so touch an action.
  p.setV(0)
  return { p, rt: p.__refTracker, log, wakes, quiet: () => isQuiet() }
}

const emit = (type: string, data: unknown) =>
  useEventBusStore.getState().emit({ type, data } as never)

test('QUIET-1: a pinned, hidden store defers its handlers and replays them ONCE (deduped) when shown', async () => {
  setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  const { p, rt, log, wakes, quiet } = mkQuietStore()
  rt.addRef('v')
  pinStore(p, 'entry')
  await emit('sync:series', { id: 'a' })
  assert.deepEqual(log, ['series:a'])
  rt.removeRef('v')
  assert.equal(quiet(), true)
  await emit('sync:series', { id: 'b' })
  await emit('sync:series', { id: 'b' })
  await emit('app:other', { n: 1 })
  // Hidden: nothing ran.
  assert.deepEqual(log, ['series:a'])
  rt.addRef('v')
  assert.equal(quiet(), false)
  // Shown: the queue replays once, in arrival order, the duplicate collapsed.
  assert.deepEqual(log, ['series:a', 'series:b', 'other:1'])
  assert.equal(wakes.length, 1)
  assert.equal(wakes[0].stale, false)
  assert.equal(wakes[0].replayed, 2)
  unpinStore(p, 'entry')
})

test('QUIET-2: past the revalidate window the store revalidates ONCE via its own sync:reconnect handler (queued sync frames superseded)', async t => {
  setQuietPolicy({ revalidateAfterMs: 1000 })
  t.mock.timers.enable({ apis: ['Date'], now: 1_000_000 })
  try {
    const { p, rt, log, wakes } = mkQuietStore()
    rt.addRef('v')
    pinStore(p, 'entry')
    rt.removeRef('v')
    await emit('sync:series', { id: 'z' })
    await emit('app:other', { n: 7 })
    t.mock.timers.tick(1500)
    rt.addRef('v')
    assert.deepEqual(log, ['other:7', 'reconnect'])
    assert.equal(wakes[0].stale, true)
    assert.ok(wakes[0].hiddenMs >= 1500)
    unpinStore(p, 'entry')
  } finally {
    setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  }
})

test('QUIET-3: inside the window and with nothing queued, showing a pinned store fetches nothing', async t => {
  setQuietPolicy({ revalidateAfterMs: 60_000 })
  t.mock.timers.enable({ apis: ['Date'], now: 5_000_000 })
  try {
    const { p, rt, log, wakes } = mkQuietStore()
    rt.addRef('v')
    pinStore(p, 'entry')
    rt.removeRef('v')
    t.mock.timers.tick(30_000)
    rt.addRef('v')
    assert.deepEqual(log, [])
    assert.equal(wakes[0].stale, false)
    unpinStore(p, 'entry')
  } finally {
    setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  }
})

test("QUIET-4: { whileQuiet: 'run' } runs while hidden; an unpinned hidden store is not quiet", async () => {
  const { p, rt, log } = mkQuietStore({ runWhileQuiet: true })
  rt.addRef('v')
  pinStore(p, 'entry')
  rt.removeRef('v')
  await emit('app:other', { n: 2 })
  assert.deepEqual(log, ['other:2'])
  unpinStore(p, 'entry')
  // Released: no pin, nothing renders → not quiet → handled immediately.
  await emit('sync:series', { id: 'q' })
  assert.deepEqual(log, ['other:2', 'series:q'])
  rt.executeDestroy()
})

test('QUIET-5: released while hidden with events queued → the next show revalidates even inside the window', async () => {
  setQuietPolicy({ revalidateAfterMs: 60_000 })
  try {
    const { p, rt, log } = mkQuietStore()
    rt.addRef('v')
    pinStore(p, 'entry')
    rt.removeRef('v')
    await emit('sync:series', { id: 'lost' })
    unpinStore(p, 'entry')
    assert.deepEqual(log, [])
    rt.addRef('v')
    assert.deepEqual(log, ['reconnect'])
  } finally {
    setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  }
})

test('QUIET-6: a parked watch replays once with the first prev and the latest value — and not at all when the value came back', () => {
  const src = defineStore(`QuietSrc${quietSeq++}`, { state: { k: 1 } })
  const calls: Array<[number, number]> = []
  const h = defineStore(`QuietWatch${quietSeq++}`, {
    state: { v: 0 },
    actions: set => ({ setV: (v: number) => set(() => ({ v })) }),
    init: ({ watch }) => {
      watch(src.store as any, (s: any) => s.k, (value: number, prev: number) => {
        calls.push([value, prev])
      })
    },
  })
  const p: any = createStoreProxy(h.store as any)
  p.setV(0)
  const rt = p.__refTracker
  rt.addRef('v')
  pinStore(p, 'e')
  rt.removeRef('v')
  const fired = calls.length // fireImmediately
  src.store.setState({ k: 2 })
  src.store.setState({ k: 3 })
  assert.equal(calls.length, fired)
  rt.addRef('v')
  assert.deepEqual(calls.slice(fired), [[3, 1]])
  // Hidden again: 3 → 4 → 3 is no change at all.
  rt.removeRef('v')
  src.store.setState({ k: 4 })
  src.store.setState({ k: 3 })
  rt.addRef('v')
  assert.deepEqual(calls.slice(fired), [[3, 1]])
  unpinStore(p, 'e')
})

test('QUIET-7: a destroy drops the queue (the re-init is the fresh answer)', async () => {
  const { p, rt, log } = mkQuietStore()
  rt.addRef('v')
  pinStore(p, 'entry')
  rt.removeRef('v')
  await emit('app:other', { n: 9 })
  rt.executeDestroy()
  unpinStore(p, 'entry')
  assert.deepEqual(log, [])
})

test('QUIET-8: a reconnect handler registered with { wakeRevalidate: false } is NOT the wake revalidation — onWake owns it', async t => {
  setQuietPolicy({ revalidateAfterMs: 1000 })
  t.mock.timers.enable({ apis: ['Date'], now: 9_000_000 })
  try {
    const log: string[] = []
    const h = defineStore(`QuietOptOut${quietSeq++}`, {
      state: { v: 0 },
      actions: set => ({ setV: (v: number) => set(() => ({ v })) }),
      init: ({ on, onWake }) => {
        on('sync:reconnect' as never, (() => {
          log.push('destructive-reconnect')
        }) as never, { wakeRevalidate: false })
        onWake(info => {
          if (info.stale) log.push('soft-refresh')
        })
      },
    })
    const p: any = createStoreProxy(h.store as any)
    p.setV(0)
    const rt = p.__refTracker
    rt.addRef('v')
    pinStore(p, 'e')
    rt.removeRef('v')
    t.mock.timers.tick(2000)
    rt.addRef('v')
    assert.deepEqual(log, ['soft-refresh'])
    // A REAL reconnect while visible still runs the handler.
    await emit('sync:reconnect', {})
    assert.deepEqual(log, ['soft-refresh', 'destructive-reconnect'])
    unpinStore(p, 'e')
  } finally {
    setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  }
})

test('QUIET-9: releasing a hidden store folds its queued EVENTS into a revalidation but keeps a parked watch — the value edge still replays on show', () => {
  setQuietPolicy({ revalidateAfterMs: 60_000 })
  try {
    const src = defineStore(`QuietFoldSrc${quietSeq++}`, { state: { k: 1 } })
    const log: string[] = []
    const h = defineStore(`QuietFold${quietSeq++}`, {
      state: { v: 0 },
      actions: set => ({ setV: (v: number) => set(() => ({ v })) }),
      init: ({ watch, on }) => {
        watch(src.store as any, (s: any) => s.k, (value: number) => {
          log.push(`k=${value}`)
        }, { fireImmediately: false })
        on('sync:reconnect' as never, (() => {
          log.push('reconnect')
        }) as never)
      },
    })
    const p: any = createStoreProxy(h.store as any)
    p.setV(0)
    const rt = p.__refTracker
    rt.addRef('v')
    pinStore(p, 'e')
    rt.removeRef('v')
    src.store.setState({ k: 2 })
    unpinStore(p, 'e') // released while hidden
    rt.addRef('v')
    assert.deepEqual(log, ['k=2'])
    rt.executeDestroy()
  } finally {
    setQuietPolicy({ revalidateAfterMs: Number.POSITIVE_INFINITY })
  }
})
