/**
 * Store-seed durability helpers (JSX-free, so they unit-test in plain Node).
 *
 * A seeded surface / overlay renders a REAL component, lets it load normally
 * (loaded cassette), then a `setup()`/`open()` seeds the transient piece through
 * the REAL store — the channel that reaches branches the GET-driven data-state
 * pass (empty/error/delayed) structurally can't. These keep the seed asserted
 * long enough to survive a late-arriving load.
 */
const tick = (ms = 80) => new Promise(r => setTimeout(r, ms))

/** Poll until `pred()` is true (store finished its loaded-cassette load), capped. */
export async function whenTrue(pred: () => boolean, tries = 60): Promise<void> {
  for (let i = 0; i < tries; i++) {
    if (pred()) return
    await tick(60)
  }
}

/**
 * Re-apply a store patch a few times over ~2.5s. Stores auto-load on init and
 * some re-subscribe, so a one-shot `setState` seed can be clobbered by a
 * late-arriving load. Re-asserting keeps the seeded branch rendered long enough
 * to be counted by the istanbul render pass.
 */
export async function holdPatch(
  apply: () => void,
  times = 26,
  gap = 185,
): Promise<void> {
  for (let i = 0; i < times; i++) {
    apply()
    await tick(gap)
  }
}

/**
 * Assert a store patch on a PERMANENT interval (every 150ms, never cleared — dies
 * on page navigation). Use for LOADING arms whose component lazy-mounts at an
 * unpredictable time under the full pass, where a fixed-duration `holdPatch` can
 * end before the slow-loading chunk first renders.
 */
export function holdForever(apply: () => void): void {
  apply()
  setInterval(apply, 150)
}
