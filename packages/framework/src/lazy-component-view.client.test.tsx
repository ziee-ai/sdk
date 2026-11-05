// @vitest-environment jsdom
//
// The first-render suspend (consumer report 2026-11-05: "that skeleton makes it
// look like flashing"): React 19's `lazy()` suspends on a type's FIRST render
// even when the import has already resolved, so a preloaded route painted its
// Suspense fallback for a frame. A loader recorded with `primeLazyLoader`
// renders its component in the FIRST commit — no fallback; an unprimed one
// keeps today's lazy path (the fallback first).
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { afterEach, describe, expect, it } from 'vitest'
import { RenderComponentLike, primeLazyLoader } from './lazy-component-view'
import { markLazyLoader } from './lazy-component'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const Page = () => <span>PAGE</span>
let host: HTMLDivElement | null = null
let root: Root | null = null
afterEach(() => {
  act(() => root?.unmount())
  host?.remove()
  host = null
  root = null
})

/** Render once and read the FIRST commit synchronously (no awaited microtask). */
function firstCommit(component: () => Promise<{ default: typeof Page }>): string {
  host = document.createElement('div')
  document.body.appendChild(host)
  root = createRoot(host)
  act(() => {
    root!.render(<RenderComponentLike component={component} fallback={<i>FALLBACK</i>} />)
  })
  return host.innerHTML
}

describe('RenderComponentLike — a primed loader never shows its fallback', () => {
  it('primed: the first commit IS the page', async () => {
    const loader = markLazyLoader(() => Promise.resolve({ default: Page }))
    await primeLazyLoader(loader)
    expect(firstCommit(loader)).toContain('PAGE')
    expect(host!.innerHTML).not.toContain('FALLBACK')
  })

  it('control — unprimed (even already resolved): the fallback paints first', async () => {
    const mod = { default: Page }
    const settled = Promise.resolve(mod)
    await settled
    const loader = markLazyLoader(() => settled)
    expect(firstCommit(loader)).toContain('FALLBACK')
  })
})
