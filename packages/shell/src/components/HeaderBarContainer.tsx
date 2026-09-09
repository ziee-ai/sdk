import { useEffect, useRef, useState } from 'react'
import { appLayoutChrome } from '../app-store-seams'
import { cn } from '@ziee/kit/lib/utils'
import { useHeaderLeftInset } from '../hooks/useHeaderLeftInset'

/**
 * SEAM: `AppLayout` is an app-registered module store the shell reads through a
 * typed view (never the app's concrete store type). The cast preserves the live
 * proxy, so `nativeScroll` stays reactive. Mirrors the AppLayout / DivScrollY
 * seams.
 */
interface HeaderBarContainerProps {
  children?: React.ReactNode
  className?: string
  style?: React.CSSProperties
}

/** Scroll distance (px) before the header starts hiding on scroll-down. */
const HIDE_THRESHOLD = 50
/** Ignore sub-threshold direction flips (momentum jitter). */
const DIRECTION_DELTA = 6
/** Debounce hide/show: once toggled, ignore further toggles for this long so an
 *  iOS rubber-band bounce (which reverses direction rapidly) can't flip-flop the
 *  header. */
const TOGGLE_COOLDOWN_MS = 250

export const HeaderBarContainer = ({
  children,
  className = '',
  style = {},
}: HeaderBarContainerProps) => {
  // Optional read: the header can render store-LESS (an isolated gallery
  // overlay, a layout-less route). `nativeScroll` is an opt-in mobile flag, so
  // fall back to a plain (non-native-scroll) header + no-op hide rather than
  // throwing.
  const appLayout = appLayoutChrome()
  const { nativeScroll } = appLayout
  const headerLeftInset = useHeaderLeftInset()

  // Native document-scroll (mobile Settings): direction-based header —
  //  • default / scrolling DOWN → position:relative → header wipes away with
  //    the page, freeing the notch region so content flows under it.
  //  • scrolling UP → position:sticky top:5 → pins back into view.
  // sticky reserves the same flow slot as relative (no reflow when toggling),
  // and top:5 dodges iOS Safari's under-notch latch (top:0..~3 latch). A z-29
  // backdrop fills the 5px gap above the pinned header.
  const [pinned, setPinned] = useState(false)
  const lastY = useRef(0)
  useEffect(() => {
    const setHeaderHidden = appLayout.setHeaderHidden
    if (!nativeScroll) {
      setPinned(false)
      setHeaderHidden(false)
      return
    }
    lastY.current = window.scrollY
    let lastToggle = 0
    // `pinned` = relative(away) vs sticky(shown); `headerHidden` = whether the
    // header is off-screen (shared so the fixed toggle button follows it).
    // Debounced so a rubber-band bounce can't flip-flop it; showing-at-top is
    // exempt (must always reveal instantly).
    const toggle = (hidden: boolean, pin: boolean) => {
      const now = performance.now()
      if (now - lastToggle < TOGGLE_COOLDOWN_MS) return
      lastToggle = now
      setPinned(pin)
      setHeaderHidden(hidden)
    }
    const onScroll = () => {
      const y = window.scrollY
      const maxY = document.documentElement.scrollHeight - window.innerHeight
      // iOS Safari reports scrollY BEYOND [0, maxY] during a rubber-band
      // overscroll, and the bounce-back reads as a direction reversal — skip it
      // so the header doesn't glitch at the top/bottom edges.
      if (y < 0 || y > maxY) {
        lastY.current = Math.max(0, Math.min(y, maxY))
        return
      }
      if (y <= HIDE_THRESHOLD) {
        setPinned(false) // at top → relative, shown (instant, no debounce)
        setHeaderHidden(false)
        lastY.current = y
        return
      }
      const dy = y - lastY.current
      if (Math.abs(dy) < DIRECTION_DELTA) return // jitter — ignore, keep lastY
      if (dy > 0) toggle(true, false) // scrolling down → hide (relative, wipes)
      else toggle(false, true) // scrolling up → show (sticky, reappears)
      lastY.current = y
    }
    window.addEventListener('scroll', onScroll, { passive: true })
    return () => window.removeEventListener('scroll', onScroll)
  }, [nativeScroll])

  return (
    <>
      {/* Backdrop panel: only while the header is shown (pinned). Fills from the
          viewport top down to the header's bottom (top:5 + safe-area + 50), sits
          BEHIND the header but ABOVE the page content, so scrolling content
          doesn't bleed through the 5px gap above the pinned header. Not rendered
          while hidden, so it never affects the under-notch behavior. */}
      {nativeScroll && pinned && (
        <div
          aria-hidden
          className="fixed inset-x-0 top-0 bg-card animate-in fade-in duration-300"
          style={{
            // top:5 + safe-area + 45px box = the header's bottom edge
            height: 'calc(env(safe-area-inset-top, 0px) + 50px)',
            zIndex: 29,
          }}
        />
      )}
      <div
      data-testid="app-header-bar"
      className={cn(
        'w-full flex px-3 border-b border-border box-border py-0',
        nativeScroll
          ? cn(
              'bg-card',
              pinned
                ? 'sticky animate-in fade-in slide-in-from-top-4 duration-300 ease-out'
                : 'relative',
            )
          : 'h-[50px] relative transition-all duration-200 ease-in-out',
        className,
      )}
      style={{
        paddingLeft: headerLeftInset,
        paddingRight: 12,
        zIndex: nativeScroll ? 30 : 2,
        ...(nativeScroll
          ? {
              // Box is 45px tall so its bottom border lands on the 50px line
              // (5px offset + 45). paddingBottom:5 shrinks the CONTENT area to
              // 40px so the content centers at safe-area+25 — matching the
              // non-native header (border at 50, content centered at 25).
              paddingTop: 'env(safe-area-inset-top, 0px)',
              paddingBottom: 5,
              height: 'calc(env(safe-area-inset-top, 0px) + 45px)',
              // top:5 (sticky) dodges Safari's under-notch latch (top:0..~3
              // latch). When relative, replicate the same 5px offset via
              // marginTop so the header content stays at safe-area+5 in BOTH
              // states — otherwise it jumps up 5px at rest and mis-aligns the
              // toggle button.
              ...(pinned ? { top: 5 } : { marginTop: 5 }),
            }
          : null),
        ...style,
      }}
    >
      {children}
      </div>
    </>
  )
}
