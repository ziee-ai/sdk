// @vitest-environment jsdom
//
// A bottom (or top) Sheet is BOUNDED, and its body can shrink — so tall content scrolls.
//
// The defect this pins: `data-[side=bottom]` and `data-[side=top]` carried `h-auto` while only
// left/right got `h-full`. The popup is a flex COLUMN whose body is `flex-1 overflow-y-auto`, and
// `flex-1` is inert under an AUTO-height parent — there is no definite height to take a remainder
// of. So a tall bottom sheet did not scroll: it grew upward, past the top of the screen, and what
// was at the top of its content became unreachable rather than merely off screen. `MobileShell`'s
// bottom "Analysis" sheet hosts `BottomArtifactsDock`, so this was the shipped path.
//
// Measured before the fix, on a real 390×800 phone viewport
// (`src-app/ui/tests/gallery-e2e/sheet-bottom-track.spec.ts`, with 3000px of content in the body):
//   sheet-content  top=-2454  height=3293  max-height=none  → 2454px above the top of the screen
//   body           flex=1/1/0%  h=3220  scrollHeight=3220                   → NO scroll track
// and after, on the same viewport and content:
//   sheet-content  top=160    height=680   max-height=680px
//   body           h=607      scrollHeight=3220                             → a real track
//
// THE FIX IS NOT `h-full`. That also stops the overflow — by making every bottom sheet
// full-screen, which is a visible regression for every consumer including the short ones that
// were always correct. A max-height caps only what is too tall; a short sheet still sizes to its
// content. The third test below is that negative control.
//
// Nor is it `min-h-0` on the body, which was the other obvious half and turned out to be inert
// here — see the last test.
//
// Why `svh` and not `vh`/`dvh`: `vh` resolves against the LARGE viewport, so on a mobile browser
// whose toolbar is showing, an 85vh sheet is taller than the visible area — the exact failure this
// is fixing, reintroduced at a smaller scale. `svh` is the small (toolbar-visible) viewport, which
// is the one that is always safe. The kit's sidebar already uses `svh` for the same reason.
//
// jsdom has no layout engine and no Tailwind, so what is asserted here is the CLASS contract —
// the mechanism by which the layout happens. The geometry proof is the gallery-e2e above; this is
// what makes a regression cheap to catch.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { cleanup, render } from '@testing-library/react'
import { Sheet } from './sheet'

beforeAll(() => {
  ;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true
  globalThis.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  } as never
})

afterEach(cleanup)

function popup(side: 'top' | 'right' | 'bottom' | 'left'): HTMLElement {
  render(
    <Sheet open data-testid="sh" title="T" side={side}>
      <div data-testid="body-child" />
    </Sheet>,
  )
  const el = document.querySelector<HTMLElement>('[data-slot="sheet-content"]')
  if (!el) throw new Error('the sheet did not render')
  return el
}

/** The kit `Sheet`'s scrolling body — the element a pane is rendered into. */
function scrollBody(): HTMLElement {
  const child = document.querySelector<HTMLElement>('[data-testid="body-child"]')
  const body = child?.parentElement
  if (!body) throw new Error('the sheet body did not render')
  return body
}

describe('Sheet bottom/top: bounded, and the body can shrink', () => {
  it.each(['bottom', 'top'] as const)('%s is capped, not auto-height', side => {
    const cls = popup(side).className
    expect(cls, `no height cap on data-[side=${side}] — a tall sheet grows off screen`).toContain(
      `data-[side=${side}]:max-h-[85svh]`,
    )
    // The negative control for THIS half: `h-auto` back alongside the cap would be harmless, but
    // `h-auto` INSTEAD of it is the defect, so the absence is asserted rather than inferred.
    expect(cls, `data-[side=${side}]:h-auto is the defect`).not.toContain(`data-[side=${side}]:h-auto`)
  })

  it.each(['bottom', 'top'] as const)('%s does NOT become full-screen', side => {
    // `h-full` would also stop the overflow and would be a regression for every consumer. The
    // gallery-e2e measures the same claim as geometry; this is the cheap half.
    expect(popup(side).className).not.toContain(`data-[side=${side}]:h-full`)
  })

  it('left/right keep their full-height contract untouched', () => {
    // They were never broken: an inset-y-0 + h-full panel has a definite height already, so their
    // `flex-1` body has always resolved. A fix that disturbed them would be a new bug.
    for (const side of ['left', 'right'] as const) {
      const cls = popup(side).className
      expect(cls).toContain(`data-[side=${side}]:h-full`)
      expect(cls).not.toContain(`data-[side=${side}]:max-h-[85svh]`)
      cleanup()
    }
  })

  it('the body is the flexible scroll container the cap resolves against', () => {
    // The two properties the cap actually depends on. NOT `min-h-0`: the obvious companion class
    // was written, measured, and removed. A flex item's automatic minimum size is its min-content
    // size only while its computed `overflow` is `visible`, and this element IS the scroll
    // container — so its automatic minimum is already 0 and it shrinks unaided. Measured at
    // 390×800 with 3000px of content and no `min-h-0`: body 607px, scrollHeight 3220 — a real
    // track. Asserting the class anyway would have certified something the layout does not use,
    // and a mutation that deleted it would have failed this file while the page stayed correct.
    popup('bottom')
    const cls = scrollBody().className.split(/\s+/)
    expect(cls, 'the body must be the flexible one — it is what consumes the capped height').toContain('flex-1')
    expect(cls, 'and the scroll container — otherwise the cap clips instead of scrolling').toContain('overflow-y-auto')
  })
})
