// @vitest-environment jsdom
//
// The first-render suspend (consumer report 2026-11-05: "that skeleton makes it
// look like flashing"): React 19's `lazy()` suspends on a type's FIRST render
// even when the import has already resolved, so a preloaded route painted its
// Suspense fallback for a frame. A loader recorded with `primeLazyLoader`
// renders its component in the FIRST commit — no fallback; an unprimed one
// keeps today's lazy path (the fallback first).
import { act, Component, type ReactNode } from 'react'
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

// fix/preload-storm: a lazy type whose load REJECTED is rejected forever (React
// caches the status on the type). The cache must not keep it, or one failed
// chunk fetch — a background warm that lost to ERR_INSUFFICIENT_RESOURCES —
// leaves that slot / route broken for the life of the page.

class Boundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false }
  static getDerivedStateFromError() {
    return { failed: true }
  }
  render() {
    return this.state.failed ? <b>BOUNDARY</b> : this.props.children
  }
}

describe('RenderComponentLike — a rejected load is not cached', () => {
  it('after a failed load, the next mount calls the loader again and renders', async () => {
    let calls = 0
    const loader = markLazyLoader(() => {
      calls += 1
      return calls === 1
        ? Promise.reject(new TypeError('Failed to fetch dynamically imported module: x.js'))
        : Promise.resolve({ default: Page })
    })
    const mount = async () => {
      host?.remove()
      act(() => root?.unmount())
      host = document.createElement('div')
      document.body.appendChild(host)
      root = createRoot(host)
      await act(async () => {
        root!.render(
          <Boundary>
            <RenderComponentLike component={loader} fallback={<i>FALLBACK</i>} />
          </Boundary>,
        )
      })
      await act(async () => {
        await new Promise(r => setTimeout(r, 0))
      })
      return host.innerHTML
    }
    const origError = console.error
    console.error = () => undefined
    try {
      expect(await mount()).toContain('BOUNDARY')
      expect(await mount()).toContain('PAGE')
      expect(calls).toBe(2)
    } finally {
      console.error = origError
    }
  })
})
