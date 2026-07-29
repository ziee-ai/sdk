import { test, beforeEach } from 'node:test'
import assert from 'node:assert/strict'
import {
  __resetStaleBuildForTests,
  installChunkLoadRecovery,
  isStaleBuild,
  markStaleBuild,
} from './chunk-recovery.ts'

/**
 * TEST-11 — the `vite:preloadError` listener.
 *
 * `vite:preloadError` only fires against a real BUILT bundle (dev has no
 * `__vitePreload`), so this drives the listener through an injected EventTarget
 * — the handler's whole contract is observable there: it marks the build stale,
 * it does NOT preventDefault, and it uninstalls cleanly.
 *
 * The must-NOT-preventDefault assertion is load-bearing, not pedantry. Vite's
 * helper ends `baseModule().catch(handlePreloadError)`, and `handlePreloadError`
 * rethrows ONLY when the event was not defaultPrevented — so preventing the
 * default makes the import promise RESOLVE WITH `undefined` instead of
 * rejecting. The caller then reads `.default` off `undefined`, the dispatcher's
 * retry never runs (nothing rejected), and the failure is silent again. An
 * earlier draft of this module did call preventDefault and the e2e's
 * "self-heals after the blip clears" leg failed with exactly that TypeError;
 * this assertion is what stops it coming back.
 */

/** Minimal EventTarget double that records listeners + preventDefault calls. */
function makeTarget() {
  const listeners = new Map<string, Set<EventListener>>()
  return {
    listeners,
    addEventListener(type: string, fn: EventListener) {
      if (!listeners.has(type)) listeners.set(type, new Set())
      listeners.get(type)!.add(fn)
    },
    removeEventListener(type: string, fn: EventListener) {
      listeners.get(type)?.delete(fn)
    },
    count(type: string) {
      return listeners.get(type)?.size ?? 0
    },
    fire(type: string, payload?: unknown) {
      let prevented = false
      const event = {
        type,
        payload,
        preventDefault: () => {
          prevented = true
        },
      } as unknown as Event
      for (const fn of listeners.get(type) ?? []) fn(event)
      return prevented
    },
  }
}

async function quiet(fn: () => void) {
  const original = console.warn
  console.warn = () => {}
  try {
    fn()
  } finally {
    console.warn = original
  }
}

beforeEach(() => __resetStaleBuildForTests())

test('TEST-11: a preloadError marks the build stale and is NOT preventDefaulted', async () => {
  const target = makeTarget()
  const uninstall = installChunkLoadRecovery(target as any)
  try {
    assert.equal(target.count('vite:preloadError'), 1)
    assert.equal(isStaleBuild(), false)

    let prevented = false
    await quiet(() => {
      prevented = target.fire('vite:preloadError', {
        message: 'Unable to preload CSS/JS',
      })
    })

    assert.equal(
      prevented,
      false,
      'the event must NOT be preventDefaulted — that would make the import promise resolve with `undefined` instead of rejecting, silently defeating the dispatcher retry AND the caller error handling',
    )
    assert.equal(
      isStaleBuild(),
      true,
      'the page is now running against a build the server no longer fully serves',
    )
  } finally {
    uninstall()
  }
})

test('TEST-11b: installing twice registers exactly ONE listener', () => {
  const target = makeTarget()
  const a = installChunkLoadRecovery(target as any)
  const b = installChunkLoadRecovery(target as any)
  try {
    assert.equal(target.count('vite:preloadError'), 1)
  } finally {
    a()
    b()
  }
})

test('TEST-11c: uninstall removes the listener', async () => {
  const target = makeTarget()
  const uninstall = installChunkLoadRecovery(target as any)
  uninstall()
  assert.equal(target.count('vite:preloadError'), 0)

  await quiet(() => target.fire('vite:preloadError'))
  assert.equal(isStaleBuild(), false, 'an uninstalled listener must not mark anything')
})

test('TEST-11d: with no event target (SSR / node) install is a harmless no-op', () => {
  const uninstall = installChunkLoadRecovery(undefined)
  assert.equal(typeof uninstall, 'function')
  uninstall()
  assert.equal(isStaleBuild(), false)
})

test('TEST-11e: markStaleBuild is idempotent and sticky', () => {
  assert.equal(isStaleBuild(), false)
  markStaleBuild()
  markStaleBuild()
  assert.equal(isStaleBuild(), true)
})
