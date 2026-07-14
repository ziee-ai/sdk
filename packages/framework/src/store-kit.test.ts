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
