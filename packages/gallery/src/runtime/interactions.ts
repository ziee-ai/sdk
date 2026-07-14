/**
 * INTERACTION RECIPES — the mechanism that renders interaction-gated states.
 *
 * ROOT CAUSE this closes: most real-world UI bugs live in INTERACTION-GATED
 * states (click-to-edit inline forms, hover-reveals, approval prompts, expanded
 * modes) that the gallery's mount-only capture NEVER renders. During the
 * gap-grind ~40 coverage-allowlist branches were excused as "interaction-gated",
 * quarantining exactly the bug-dense states. A recipe drives REAL user actions
 * (click testid X, type, focus, hover) AFTER a surface mounts, so:
 *   - the capture pipeline shoots the resulting state (`slug__<name>.png`);
 *   - the branch-coverage pass counts the now-exercised branch as COVERED
 *     (a `via: 'interaction'` delivery in stateCoverage), instead of allow-listing it.
 *
 * A recipe's `steps` receive a {@link PageDriver}. The SAME recipe runs in TWO
 * contexts off ONE in-page driver:
 *   - Playwright captures / the coverage pass navigate to `?surface=X&interact=name`
 *     — the frame runs the recipe on mount, marks `data-gallery-interact-done`, and
 *     the pass screenshots / reads `window.__coverage__`;
 *   - a human eyeballing the vite gallery opens the same URL and watches it drive.
 * There is NO separate Playwright-side driver to keep in sync — the driver is
 * pure DOM, so it is deterministic and portable.
 */

/** Real user actions a recipe can drive, addressed by `data-testid`. */
export interface PageDriver {
  /** Click the element with this testid (waits for it first). */
  click(testid: string): Promise<void>
  /** Focus + type `text` into an input/textarea (fires React's onChange). */
  type(testid: string, text: string): Promise<void>
  /** Programmatically focus the element (drives :focus / focus-visible rings). */
  focus(testid: string): Promise<void>
  /** Fire pointer/mouse-enter so JS hover-reveal handlers run. */
  hover(testid: string): Promise<void>
  /** Resolve when the testid is present (or throw after `timeoutMs`). */
  waitFor(testid: string, timeoutMs?: number): Promise<HTMLElement>
  /** Best-effort: resolve the FIRST testid that appears within `timeoutMs`. */
  waitForAny(testids: string[], timeoutMs?: number): Promise<HTMLElement | null>
  /** Sleep `ms`. */
  wait(ms: number): Promise<void>
  /** Query a testid now (null if absent). Escape hatch for conditional steps. */
  query(testid: string): HTMLElement | null
}

/** A named interaction to drive after a surface mounts. */
export interface InteractionRecipe {
  /** Recipe name → `?interact=<name>` + screenshot suffix `slug__<name>.png`.
   *  Kebab-case, UNIQUE within the surface. */
  name: string
  /** One-line note about the interaction / branch this exercises (for reports). */
  note?: string
  /** Drive real user actions once the surface has mounted + settled. */
  steps: (d: PageDriver) => Promise<void>
}

/** An entry class that can carry interaction recipes. */
export interface HasInteractions {
  interactions?: InteractionRecipe[]
}

const sleep = (ms: number) => new Promise<void>(r => setTimeout(r, ms))

function sel(testid: string): string {
  return `[data-testid="${testid.replace(/"/g, '\\"')}"]`
}

function findByTestId(testid: string): HTMLElement | null {
  // Query the whole document: overlays/dialogs portal to <body>, outside the frame.
  return document.querySelector<HTMLElement>(sel(testid))
}

async function waitForTestId(
  testid: string,
  timeoutMs = 5000,
): Promise<HTMLElement> {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const el = findByTestId(testid)
    if (el) return el
    if (Date.now() > deadline)
      throw new Error(`interaction: testid "${testid}" not found within ${timeoutMs}ms`)
    await sleep(60)
  }
}

/**
 * Set an <input>/<textarea> value the way React expects: via the native value
 * setter (bypassing React's overridden one) + a bubbling `input` event, so the
 * controlled component's onChange fires and state updates.
 */
function setReactInputValue(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto =
    el instanceof HTMLTextAreaElement
      ? window.HTMLTextAreaElement.prototype
      : window.HTMLInputElement.prototype
  const setter = Object.getOwnPropertyDescriptor(proto, 'value')?.set
  setter?.call(el, value)
  el.dispatchEvent(new Event('input', { bubbles: true }))
  el.dispatchEvent(new Event('change', { bubbles: true }))
}

/** The single in-page driver — pure DOM, portable across capture + manual view. */
export function makeDomDriver(): PageDriver {
  return {
    query: findByTestId,
    waitFor: waitForTestId,
    wait: sleep,
    async waitForAny(testids, timeoutMs = 5000) {
      const deadline = Date.now() + timeoutMs
      for (;;) {
        for (const id of testids) {
          const el = findByTestId(id)
          if (el) return el
        }
        if (Date.now() > deadline) return null
        await sleep(60)
      }
    },
    async click(testid) {
      const el = await waitForTestId(testid)
      // A native click drives onClick handlers (buttons/rows); pointer events
      // first so components keyed on pointerdown (Base-UI triggers) also respond.
      el.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }))
      el.dispatchEvent(new PointerEvent('pointerup', { bubbles: true }))
      el.click()
      await sleep(80)
    },
    async type(testid, text) {
      const el = await waitForTestId(testid)
      const input =
        el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement
          ? el
          : el.querySelector<HTMLInputElement | HTMLTextAreaElement>('input, textarea')
      if (!input) throw new Error(`interaction: no input under "${testid}"`)
      input.focus()
      setReactInputValue(input, text)
      await sleep(60)
    },
    async focus(testid) {
      const el = await waitForTestId(testid)
      const focusable =
        el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement
          ? el
          : el.querySelector<HTMLElement>('input, textarea, button, [tabindex]') ?? el
      // Hint keyboard modality so :focus-visible rings paint (heuristic browsers
      // suppress the ring for programmatic/mouse focus otherwise).
      document.dispatchEvent(
        new KeyboardEvent('keydown', { key: 'Tab', bubbles: true }),
      )
      focusable.focus()
      await sleep(80)
    },
    async hover(testid) {
      const el = await waitForTestId(testid)
      for (const type of ['pointerover', 'pointerenter', 'mouseover', 'mouseenter']) {
        el.dispatchEvent(
          type.startsWith('pointer')
            ? new PointerEvent(type, { bubbles: true })
            : new MouseEvent(type, { bubbles: true }),
        )
      }
      await sleep(80)
    },
  }
}

/**
 * Run one recipe against the live DOM, then stamp `data-gallery-interact-done`
 * on <body> (the signal the capture/coverage passes wait on). Errors are logged,
 * not thrown — a broken recipe must not blank the surface or crash the pass; the
 * screenshot then shows the un-driven state and the finding is visible.
 */
export async function runInteraction(
  recipe: InteractionRecipe,
  settleMs = 400,
): Promise<void> {
  const body = document.body
  // Re-entry lock (set SYNCHRONOUSLY, before the first await): React StrictMode
  // double-invokes the mount effect in dev (setup → cleanup → setup), so
  // useRunInteraction calls this twice. Without the lock a TOGGLE recipe (open a
  // Popover/menu) fires twice and ends CLOSED — the exact state we were driving to
  // reveal. The lock lets only the first invocation run; the second bails.
  if (body.getAttribute('data-gallery-interact-lock') === recipe.name) return
  body.setAttribute('data-gallery-interact-lock', recipe.name)
  body.setAttribute('data-gallery-interact', recipe.name)
  try {
    await sleep(settleMs)
    await recipe.steps(makeDomDriver())
  } catch (err) {
    console.error(`[gallery-interaction ${recipe.name}]`, err)
  } finally {
    body.setAttribute('data-gallery-interact-done', recipe.name)
  }
}

/**
 * Read the requested `?interact=<name>` from the URL, resolve it against an
 * entry's `interactions`, and return the matching recipe (or undefined).
 */
export function requestedInteraction(
  interactions: InteractionRecipe[] | undefined,
): InteractionRecipe | undefined {
  if (!interactions?.length) return undefined
  const name =
    typeof window !== 'undefined'
      ? new URLSearchParams(window.location.search).get('interact')
      : null
  if (!name) return undefined
  return interactions.find(r => r.name === name)
}

import { useEffect } from 'react'

/**
 * Frame hook: after mount, if the URL requests one of this entry's interactions,
 * drive it. `deps` lets the frame delay until its own mount-time seed has run
 * (pass the entry object). Safe to call with `undefined` interactions (no-op).
 */
export function useRunInteraction(
  interactions: InteractionRecipe[] | undefined,
  settleMs = 400,
): void {
  useEffect(() => {
    const recipe = requestedInteraction(interactions)
    if (recipe) void runInteraction(recipe, settleMs)
    // Recipes are keyed off the URL (immutable per page-load); run once on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}

/** Flat `{ slug, name, note }` manifest across every interaction-bearing entry —
 *  the single list the Node capture/coverage tools enumerate through. */
export interface InteractionManifestEntry {
  slug: string
  name: string
  note?: string
}

export function buildInteractionManifest(
  entries: { slug: string; interactions?: InteractionRecipe[] }[],
): InteractionManifestEntry[] {
  const out: InteractionManifestEntry[] = []
  for (const e of entries)
    for (const r of e.interactions ?? [])
      out.push({ slug: e.slug, name: r.name, note: r.note })
  return out
}
