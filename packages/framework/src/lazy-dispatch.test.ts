import { test } from 'node:test'
import assert from 'node:assert/strict'
import { createLazyDispatcher } from './lazy-dispatch.ts'

/**
 * TEST-6 (acceptance, INV-2) / TEST-7 / TEST-8 — the lazy-action dispatcher's
 * failure policy.
 *
 * INV-2, verbatim from the design source: "a transient blip doesn't permanently
 * brick a lazy action for the session."
 *
 * The shipped dispatcher took ONE combined loader and applied the deterministic
 * policy to every failure: after the second rejection the promise was memoized
 * forever, so a single chunk 404 (which is what ANY deploy while a tab is open
 * produces) bricked that action until the tab was reloaded. TEST-6 asserts the
 * PROPERTY the design promises — a later dispatch succeeds once the blip clears
 * — and would fail under that policy regardless of how it was implemented.
 *
 * TEST-7 is the deliberate negative control: relaxing the import policy must NOT
 * relax the FACTORY policy, whose "an authoring bug must fail fast, not loop
 * forever" rationale is the reason the memoization exists at all.
 */

test('TEST-6: a transient import failure does NOT brick the action for the session', async () => {
  let imports = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      // Fail every attempt of the FIRST dispatch (1 + 2 retries = 3), then
      // recover — the shape of a blip that outlasts one user action.
      if (imports <= 3) {
        throw new TypeError(
          'Failed to fetch dynamically imported module: /assets/x-abc123.js',
        )
      }
      return { default: () => (arg: string) => `ok:${arg}` }
    },
    (m: any) => m.default(),
  )

  // First user action: fails, loudly.
  await assert.rejects(() => dispatch('a'), /Failed to fetch dynamically imported/)
  assert.equal(imports, 3, 'one dispatch must make 1 + MAX_IMPORT_RETRIES attempts')

  // Second user action (no page reload): must re-import and succeed.
  assert.equal(await dispatch('a'), 'ok:a')
})

test('TEST-6b: the rejection is never memoized, however many times it fails', async () => {
  let imports = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      throw new Error('chunk gone')
    },
    (m: any) => m.default(),
  )

  for (let i = 0; i < 3; i++) {
    await assert.rejects(() => dispatch(), /chunk gone/)
  }
  // 3 dispatches × 3 attempts each. If the rejection were memoized after the
  // budget (the shipped behaviour) this would stop at 2.
  assert.equal(imports, 9)
})

test('TEST-6c: a NULLISH module namespace is treated as a failed import, not a factory bug', async () => {
  // Vite's `__vitePreload` resolves with `undefined` when a `vite:preloadError`
  // listener calls preventDefault(). Naively that reaches the build stage, dies
  // with "Cannot read properties of undefined", and gets the DETERMINISTIC
  // memoize-forever policy — the exact silent, session-fatal failure this change
  // removes. It must be classified as a (retryable, never-memoized) import
  // failure instead.
  let imports = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      return imports <= 3 ? (undefined as any) : { default: () => () => 'ok' }
    },
    (m: any) => m.default(),
  )

  await assert.rejects(() => dispatch(), /resolved with no module/)
  assert.equal(imports, 3, 'a nullish namespace must be RETRIED like any import failure')
  assert.equal(await dispatch(), 'ok', '…and must not be memoized')
})

test('TEST-7: a deterministic FACTORY throw is still memoized (fail fast, no loop)', async () => {
  let imports = 0
  let builds = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      return { default: () => {} }
    },
    () => {
      builds++
      throw new Error('action factory is broken')
    },
  )

  for (let i = 0; i < 5; i++) {
    await assert.rejects(() => dispatch(), /action factory is broken/)
  }

  // One retry (MAX_BUILD_RETRIES = 1), then the rejection is memoized: the
  // module is not re-imported and the factory is not re-run. A component that
  // dispatches from a render/effect must not spin here.
  assert.equal(builds, 2, 'the factory must run exactly 1 + MAX_BUILD_RETRIES times')
  assert.equal(imports, 2, 'a memoized factory failure must not keep re-importing')
})

test('TEST-8: the happy path memoizes exactly once across many dispatches', async () => {
  let imports = 0
  let builds = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      return { default: (n: number) => (x: number) => x + n }
    },
    (m: any) => m.default(10),
  )

  assert.equal(await dispatch(1), 11)
  assert.equal(await dispatch(2), 12)
  assert.equal(await dispatch(3), 13)
  assert.equal(imports, 1)
  assert.equal(builds, 0) // builds is only touched by the failing variant above
})

test('TEST-8b: preload() warms the chunk WITHOUT invoking the action', async () => {
  let imports = 0
  let invocations = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      return {
        default: () => () => {
          invocations++
          return 'called'
        },
      }
    },
    (m: any) => m.default(),
  )

  await dispatch.preload()
  assert.equal(imports, 1)
  assert.equal(invocations, 0, 'preload must not invoke the action')

  assert.equal(await dispatch(), 'called')
  assert.equal(imports, 1, 'the preloaded chunk must be reused, not re-imported')
  assert.equal(invocations, 1)
})

test('TEST-8c: concurrent dispatches share ONE import', async () => {
  let imports = 0
  const dispatch = createLazyDispatcher(
    async () => {
      imports++
      return { default: () => (x: string) => x }
    },
    (m: any) => m.default(),
  )

  const [a, b, c] = await Promise.all([dispatch('a'), dispatch('b'), dispatch('c')])
  assert.deepEqual([a, b, c], ['a', 'b', 'c'])
  assert.equal(imports, 1)
})
