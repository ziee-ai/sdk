import React, {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  type ComponentType,
} from 'react'
import { useLocation } from 'react-router-dom'
import { useWindowMinSize } from '../hooks/useWindowMinSize'
import { cn } from '@ziee/kit/lib/utils'
// shadcn primitives (not the kit wrapper, which forces a visible title header +
// padded scroll body) so the raw LeftSidebar can fill the panel edge-to-edge.
import { Sheet, SheetContent, SheetTitle } from '@ziee/kit/shadcn/sheet'
import { useMetaThemeColor } from '../theme/themeColor'
import 'overlayscrollbars/overlayscrollbars.css'
import { Stores } from '@ziee/framework/stores'
import { LazyComponentRenderer } from '../components/LazyComponentRenderer'

/**
 * SEAM: `AppLayout` is an app-registered module store the shell reads through a
 * typed view (never the app's concrete store type). Runtime is identical to
 * reading the `AppLayout` store directly — the cast preserves the live proxy, so
 * `isSidebarCollapsed` / `nativeScroll` stay reactive. Mirrors DivScrollY.
 */
interface AppLayoutSeam {
  isSidebarCollapsed: boolean
  nativeScroll: boolean
  $: { sidebarWidth: number }
  setSidebarCollapsed: (collapsed: boolean) => void
  setSidebarWidth: (width: number) => void
  setMainContentWidth: (width: number) => void
}

/**
 * PLATFORM-VARIANT SEAM: `LeftSidebar` + `SidebarToggleButton` carry app-side
 * `.desktop.tsx` variants that the desktop `localOverridePlugin` swaps ONLY at a
 * `@/`-prefixed import site. A package's internal imports are relative and would
 * always resolve the WEB variant on desktop, so the shell does NOT own these two
 * leaves — the app injects them here via its own `@/`-imports (swapped at the
 * app's injection site), keeping the existing override mechanism untouched.
 */
export interface AppLayoutProps {
  children: React.ReactNode
  LeftSidebar: ComponentType
  SidebarToggleButton: ComponentType
}

/**
 * AppLayout - Main application layout with sidebar
 *
 * Sidebar items are registered via the slot system:
 * - sidebarNavigation: Main navigation items
 * - sidebarTools: Tools/settings items
 * - sidebarPrimaryActions: Action buttons at top
 * - sidebarRecent: Recent items (middle section)
 * - sidebarBottom: Below tools (e.g., download indicator)
 * - sidebarFooter: Footer section (e.g., user profile)
 */
export function AppLayout({
  children,
  LeftSidebar,
  SidebarToggleButton,
}: AppLayoutProps) {
  const appLayout = (Stores as unknown as { AppLayout: AppLayoutSeam })
    .AppLayout
  // The app shell is bg-card, but the iOS status/nav bars sample the CANVAS —
  // the <body> background — which is --background app-wide. The shell covers the
  // body, so painting the body --card changes nothing visible; it only makes the
  // safe-area bands / overscroll gutter (and the sampled bar color) match the
  // shell instead of showing the darker --background. Restored on teardown so
  // the blank/login layout keeps its --background body. (theme-color is set too,
  // for the Safari-chrome path.)
  useLayoutEffect(() => {
    const body = document.body
    const prev = body.style.backgroundColor
    body.style.backgroundColor = 'var(--card)'
    return () => {
      body.style.backgroundColor = prev
    }
  }, [])
  useMetaThemeColor('--card')
  const { isSidebarCollapsed, nativeScroll } = appLayout
  const { slots } = Stores.ModuleSystem
  const appBanners = [...(slots.get('appBanners') || [])].sort(
    (a, b) => (a.order ?? 0) - (b.order ?? 0),
  )
  const windowMinSize = useWindowMinSize()

  const sidebarRef = useRef<HTMLDivElement>(null)
  const spacerRef = useRef<HTMLDivElement>(null)
  const mainContentRef = useRef<HTMLDivElement>(null)
  // Seed the local ref from the persistent store. Each route's
  // `*Layout` component mounts its OWN `<AppLayout>` instance, so a
  // plain `useRef(200)` reset the sidebar width every time the user
  // navigated. The ref stays for fast drag-time writes (no React
  // re-renders), and we sync back to the store on drag end so the
  // next mount picks up the same width.
  const currentWidth = useRef(appLayout.$.sidebarWidth)

  const MIN_WIDTH = 200
  const MAX_WIDTH = 400
  // Collapsed sidebar fully disappears on desktop (mobile already does
  // via translateX(-100%) below). The toggle button lives outside the
  // sidebar (SidebarToggleButton, fixed-positioned) so reopening still
  // works at any width.
  const COLLAPSED_WIDTH = 0

  // Single source of truth for the sidebar's transition string.
  // Both the React-managed style prop below AND every imperative
  // `sidebarRef.current.style.transition = ...` write must use this
  // exact value when re-enabling transitions — otherwise the
  // transition list drops properties (notably `transform`, which
  // drives the xs slide-in) and the next state change snaps
  // instantly instead of animating.
  const SIDEBAR_TRANSITION =
    'width 200ms ease-out, transform 200ms ease-out, box-shadow 200ms ease-out'
  const SPACER_TRANSITION = 'all 200ms ease-out'

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()

      const handleMouseMove = (e: MouseEvent) => {
        const newWidth = e.clientX

        if (spacerRef.current) {
          spacerRef.current.style.transition = 'none'
        }
        if (sidebarRef.current) {
          sidebarRef.current.style.transition = 'none'
        }

        if (newWidth < MIN_WIDTH / 2) {
          if (spacerRef.current) {
            spacerRef.current.style.transition = SPACER_TRANSITION
          }
          // Clear the imperative width override so React's next
          // re-render (with `width: currentWidth.current`) starts
          // the CSS transition from the user's settled width
          // instead of whatever narrow drag value we just wrote.
          // Without this, the sidebar would visibly grow back to
          // full width while sliding left.
          if (sidebarRef.current) {
            sidebarRef.current.style.width = ''
          }
          appLayout.setSidebarCollapsed(true)
        } else if (newWidth >= MIN_WIDTH && newWidth <= MAX_WIDTH) {
          // If coming from collapsed state, re-enable transition for smooth expand
          if (isSidebarCollapsed) {
            if (spacerRef.current) {
              spacerRef.current.style.transition = SPACER_TRANSITION
            }

            setTimeout(() => {
              appLayout.setSidebarCollapsed(false)
              currentWidth.current = newWidth
              if (sidebarRef.current) {
                sidebarRef.current.style.width = `${newWidth}px`
              }
              if (spacerRef.current) {
                spacerRef.current.style.width = `${newWidth}px`
              }

              // Resume no-transition dragging after expand
              setTimeout(() => {
                if (sidebarRef.current) {
                  sidebarRef.current.style.transition = 'none'
                }
                if (spacerRef.current) {
                  spacerRef.current.style.transition = 'none'
                }
              }, 300) // Wait for transition to complete
            }, 10)
          } else {
            // Disable the transition for smooth dragging
            appLayout.setSidebarCollapsed(false)
            currentWidth.current = newWidth
            if (sidebarRef.current) {
              sidebarRef.current.style.width = `${newWidth}px`
            }
            if (spacerRef.current) {
              spacerRef.current.style.width = `${newWidth}px`
            }
          }
        } else if (newWidth > MAX_WIDTH) {
          currentWidth.current = MAX_WIDTH
          if (sidebarRef.current) {
            sidebarRef.current.style.width = `${MAX_WIDTH}px`
          }
          if (spacerRef.current) {
            spacerRef.current.style.width = `${MAX_WIDTH}px`
          }
        }
      }

      const handleMouseUp = () => {
        if (spacerRef.current) {
          spacerRef.current.style.transition = SPACER_TRANSITION
        }
        if (sidebarRef.current) {
          sidebarRef.current.style.transition = SIDEBAR_TRANSITION
        }

        // Persist the final width so it survives route navigation.
        appLayout.setSidebarWidth(currentWidth.current)

        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
    },
    [MIN_WIDTH, MAX_WIDTH, isSidebarCollapsed],
  )

  useEffect(() => {
    if (windowMinSize.sm) {
      appLayout.setSidebarCollapsed(true)
    }
  }, [windowMinSize.sm])

  // On mobile the sidebar is a full-screen Sheet (overlay = fixed inset-0).
  // Navigation is triggered from inside that Sheet, but a route change alone
  // doesn't close it — so its transparent scrim would stay over the new page
  // and swallow every tap. Collapse on each pathname change so the Sheet closes
  // when the user picks a destination. No-op on desktop (persistent sidebar).
  const location = useLocation()
  useEffect(() => {
    if (windowMinSize.sm) {
      appLayout.setSidebarCollapsed(true)
    }
  }, [location.pathname])

  // When the xs threshold flips, the sidebar's `width` and
  // `transform` both change in the same commit. With our unified
  // SIDEBAR_TRANSITION, those changes animate over 200ms and the
  // sidebar briefly appears mid-slide (width-shrink + slide-off
  // happening simultaneously). Suppress the transition for that
  // single commit so the mode-flip snaps; transitions resume the
  // next frame for normal collapse/expand animations.
  const prevXsRef = useRef(windowMinSize.sm)
  useLayoutEffect(() => {
    if (prevXsRef.current === windowMinSize.sm) return
    prevXsRef.current = windowMinSize.sm
    const sidebarEl = sidebarRef.current
    const spacerEl = spacerRef.current
    if (sidebarEl) sidebarEl.style.transition = 'none'
    if (spacerEl) spacerEl.style.transition = 'none'
    // Force a reflow so the no-transition style is committed
    // before the next style change.
    if (sidebarEl) void sidebarEl.offsetHeight
    requestAnimationFrame(() => {
      if (sidebarRef.current) {
        sidebarRef.current.style.transition = SIDEBAR_TRANSITION
      }
      if (spacerRef.current) {
        spacerRef.current.style.transition = SPACER_TRANSITION
      }
    })
  }, [windowMinSize.sm])

  // While the viewport is CROSSING from desktop into xs, `isSidebarCollapsed`
  // still holds the desktop "expanded" (false) value for one render — feeding
  // that as open=true would mount the mobile Sheet OPEN and orphan its overlay
  // (an invisible fixed inset-0 scrim that then eats every click on the page).
  // `prevXsRef` still reads the previous (desktop) value during this render, so
  // force the Sheet closed on that frame; the collapse effect above settles the
  // flag immediately after.
  const mobileSidebarOpen =
    !isSidebarCollapsed && !(windowMinSize.sm && !prevXsRef.current)

  // Touch swipe-left to collapse the mobile sidebar Sheet: the panel follows the
  // finger and, released past ~35%/120px, closes; otherwise it snaps back.
  // Vertical-dominant gestures are ignored so the nav list still scrolls.
  const sheetSwipe = useRef<{
    x: number
    y: number
    active: boolean
    dx: number
    el: HTMLElement
  } | null>(null)
  const onSheetTouchStart = (e: React.TouchEvent<HTMLDivElement>) => {
    if (e.touches.length !== 1) return
    // Translate the panel itself (not e.currentTarget) so the gesture works
    // whether it starts on the panel OR the mask overlay.
    const panel = document.getElementById('app-sidebar')
    if (!panel) return
    const t = e.touches[0]
    sheetSwipe.current = { x: t.clientX, y: t.clientY, active: false, dx: 0, el: panel }
  }
  const onSheetTouchMove = (e: React.TouchEvent<HTMLDivElement>) => {
    const s = sheetSwipe.current
    if (!s) return
    const t = e.touches[0]
    const dx = t.clientX - s.x
    const dy = t.clientY - s.y
    if (!s.active) {
      if (Math.abs(dx) < 8 && Math.abs(dy) < 8) return
      if (Math.abs(dy) > Math.abs(dx)) {
        sheetSwipe.current = null
        return
      }
      s.active = true
    }
    s.dx = dx
    // Left sheet closes on a leftward swipe → only follow negative dx.
    s.el.style.transition = 'none'
    s.el.style.transform = `translateX(${Math.min(0, dx)}px)`
  }
  const onSheetTouchEnd = () => {
    const s = sheetSwipe.current
    sheetSwipe.current = null
    if (!s || !s.active) return
    s.el.style.transition = ''
    const width = s.el.getBoundingClientRect().width
    s.el.style.transform = ''
    if (-s.dx > Math.min(width * 0.35, 120)) {
      appLayout.setSidebarCollapsed(true)
    }
  }

  // Touch swipe-RIGHT anywhere on the page opens the (collapsed) mobile sidebar.
  // Skipped while a drawer / dialog / sheet is open (their own gestures own the
  // touch) and on desktop, where the sidebar is persistent.
  const pageSwipe = useRef<{ x: number; y: number; active: boolean } | null>(null)
  const onPageTouchStart = (e: React.TouchEvent) => {
    if (!windowMinSize.sm || !isSidebarCollapsed || e.touches.length !== 1) return
    // Any open app Drawer / Dialog / Sheet / AlertDialog owns the touch — its
    // own swipe-to-close handles it, so don't also trigger sidebar-open. Match
    // the Drawer on its stable data-slot (NOT its testid, which callers override).
    if (
      document.querySelector(
        '[data-slot="layout-drawer"], [data-slot="dialog-content"], [data-slot="sheet-content"], [role="alertdialog"]',
      )
    )
      return
    // Don't hijack a horizontal scroller (e.g. the chat-input file list): if the
    // touch starts inside an element that can scroll horizontally, let it scroll.
    for (
      let el = e.target as HTMLElement | null;
      el && el.id !== 'main-content';
      el = el.parentElement
    ) {
      const cs = getComputedStyle(el)
      const scrollableX =
        cs.overflowX === 'auto' ||
        cs.overflowX === 'scroll' ||
        el.hasAttribute('data-overlayscrollbars-viewport')
      if (scrollableX && el.scrollWidth > el.clientWidth + 1) return
    }
    const t = e.touches[0]
    pageSwipe.current = { x: t.clientX, y: t.clientY, active: false }
  }
  const onPageTouchMove = (e: React.TouchEvent) => {
    const s = pageSwipe.current
    if (!s) return
    const t = e.touches[0]
    const dx = t.clientX - s.x
    const dy = t.clientY - s.y
    if (!s.active) {
      if (Math.abs(dx) < 10 && Math.abs(dy) < 10) return
      // Vertical-dominant or leftward → not an open gesture.
      if (Math.abs(dy) > Math.abs(dx) || dx < 0) {
        pageSwipe.current = null
        return
      }
      s.active = true
    }
    if (dx > 70) {
      pageSwipe.current = null
      appLayout.setSidebarCollapsed(false)
    }
  }
  const onPageTouchEnd = () => {
    pageSwipe.current = null
  }

  // ResizeObserver to listen to main content width changes
  useEffect(() => {
    const mainContentElement = mainContentRef.current
    if (!mainContentElement) return

    const resizeObserver = new ResizeObserver(entries => {
      for (const entry of entries) {
        const { width } = entry.contentRect
        appLayout.setMainContentWidth(Math.round(width))
      }
    })

    resizeObserver.observe(mainContentElement)

    return () => {
      resizeObserver.disconnect()
    }
  }, [])

  useEffect(() => {
    //set root document background color based on theme
    const root = document.documentElement
    root.style.backgroundColor = 'var(--card)'
  }, [])

  // Visual viewport listener for mobile keyboard adjustments
  //
  // The previous version wrote `document.body.style.height` AND forced
  // `document.documentElement.scrollTop = 0` on EVERY resize event.
  // iOS Safari fires `resize` continuously while the keyboard is
  // animating in/out, so:
  //   * the unconditional scrollTop reset yanked the user to the top
  //     mid-conversation every time they tapped an input;
  //   * the body height write competed with the global app-root
  //     `height: 100dvh` rule in index.css, causing layout thrash.
  //
  // Fix (audit 02 B-4): only write body height when the viewport has
  // actually shrunk by more than ~100px from window.innerHeight (a
  // keyboard-open heuristic). Skip the scrollTop reset entirely —
  // there's no UX justification for forcing scroll position on
  // keyboard show, and the cost (lost scroll position) is real.
  useEffect(() => {
    if (!window.visualViewport) return

    const KEYBOARD_HEURISTIC_PX = 100
    const updateBodyHeight = () => {
      if (!window.visualViewport) return
      const vv = window.visualViewport
      const keyboardOpen = window.innerHeight - vv.height > KEYBOARD_HEURISTIC_PX
      if (keyboardOpen) {
        // Tell the layout to fit the visible area above the keyboard.
        document.body.style.height = `${vv.height}px`
      } else {
        // Keyboard is closed; let CSS (100dvh) take over again.
        document.body.style.height = ''
      }
    }

    updateBodyHeight()
    window.visualViewport.addEventListener('resize', updateBodyHeight)

    return () => {
      window.visualViewport?.removeEventListener('resize', updateBodyHeight)
    }
  }, [])

  // Mobile sidebar a11y (focus trap, scroll lock, Escape-to-close) is now
  // provided by the Sheet primitive that hosts the mobile sidebar — no manual
  // effect needed.

  return (
    <div className={cn(
      'w-screen flex bg-card',
      // Native document-scroll (opt-in): relax the fixed-height/overflow clamp
      // so the window scrolls. Default path is byte-identical to before.
      nativeScroll ? 'min-h-dvh overflow-x-clip' : 'h-full overflow-hidden',
    )}>
      {/* Keyboard skip link — first focusable element; jumps past the
          sidebar nav straight to the main content landmark. */}
      <a
        href="#main-content"
        className="sr-only focus:not-sr-only focus:absolute focus:z-50 focus:top-2 focus:left-2 focus:px-3 focus:py-1 focus:rounded focus:bg-card focus:shadow"
      >
        Skip to content
      </a>
      {/* Mobile sidebar — a Sheet (Base-UI Dialog) that PORTALS to <body>.
          Portaling out of the app-shell is what keeps iOS from latching the
          toolbar/safe-area when the overlay opens (the old in-shell fixed
          overlay did). The Sheet provides the backdrop, focus trap, scroll
          lock, Escape-to-close and slide-in — replacing the custom mask +
          focus-trap effect that used to live here. */}
      {windowMinSize.sm && (
        <Sheet
          open={mobileSidebarOpen}
          onOpenChange={(o) => appLayout.setSidebarCollapsed(!o)}
        >
          <SheetContent
            side="left"
            showCloseButton={false}
            id="app-sidebar"
            data-testid="app-sidebar"
            className="w-[250px] max-w-[calc(100vw-24px)] gap-0 p-0 bg-background shadow-none border-foreground/10"
            onTouchStart={onSheetTouchStart}
            onTouchMove={onSheetTouchMove}
            onTouchEnd={onSheetTouchEnd}
          >
            <SheetTitle className="sr-only">Navigation menu</SheetTitle>
            <LeftSidebar />
          </SheetContent>
        </Sheet>
      )}

      {/* Desktop sidebar — persistent, resizable panel (absolute, in-flow via
          the spacer below). Collapse slides it off-screen via translateX. */}
      {!windowMinSize.sm && (
        <div
          ref={sidebarRef}
          id="app-sidebar"
          data-testid="app-sidebar"
          data-allow-custom-color
          style={{
            position: 'absolute',
            top: 0,
            left: 0,
            height: '100%',
            overflow: 'hidden',
            zIndex: 1,
            width: currentWidth.current,
            // -100% - 1px pushes the right border fully off-screen when collapsed.
            transform: isSidebarCollapsed
              ? 'translateX(calc(-100% - 1px))'
              : 'translateX(0)',
            transition: SIDEBAR_TRANSITION,
          }}
        >
          <LeftSidebar />
        </div>
      )}

      <SidebarToggleButton />

      {/* Spacer div for layout */}
      <div
        ref={spacerRef}
        className="flex-shrink-0 z-2 pointer-events-none"
        style={
          windowMinSize.sm
            ? {
                width: 0,
              }
            : {
                width: isSidebarCollapsed
                  ? `${COLLAPSED_WIDTH}px`
                  : `${currentWidth.current}px`,
                transition: 'all 200ms ease-out', // Default transition, overridden during dragging
              }
        }
      />

      {/* Main Content Area */}
      {/* min-w-0: as a flex-1 item in the row shell, `main` defaults to
          min-width:auto and would refuse to shrink below its content's
          intrinsic width (long unbreakable card text → horizontal overflow).
          min-w-0 lets it track the viewport so inner content truncates. */}
      <main className={cn(
        'flex-1 min-w-0 flex flex-col relative bg-card',
        nativeScroll ? 'overflow-x-clip' : 'overflow-hidden',
      )}>
        {/* App-wide banners (e.g. the admin "update available" notice).
            Contributed via the `appBanners` slot, so bundles that don't load a
            contributor (e.g. desktop drops server-update) render nothing. */}
        {appBanners.map((b) => (
          <LazyComponentRenderer key={b.id} component={b.component} />
        ))}
        {/* Content */}
        <div className={cn('flex-1 min-w-0 relative', nativeScroll ? 'overflow-x-clip' : 'overflow-hidden')}>
          <section
            ref={mainContentRef}
            id="main-content"
            tabIndex={-1}
            onTouchStart={onPageTouchStart}
            onTouchMove={onPageTouchMove}
            onTouchEnd={onPageTouchEnd}
            className={cn(
              'w-full relative',
              // overflow-x-clip: clip horizontal overflow (no x-scroll / no
              // feedback into the settings two-pane) while keeping overflow-y
              // visible for document scroll.
              nativeScroll ? 'overflow-x-clip' : 'h-full overflow-hidden',
            )}
          >
            {children}
          </section>
        </div>
        {!isSidebarCollapsed && (
          <div
            // Generic semantic hook for build overrides (e.g. the
            // desktop-only floating-sidebar override) that need to
            // forward synthetic mousedowns into this handler. No
            // styling depends on it; the attribute is platform-blind.
            data-sidebar-resize-handle=""
            data-testid="layout-sidebar-resize-handle"
            className="absolute top-0 left-0 w-1 h-full cursor-col-resize z-3"
            onMouseDown={handleMouseDown}
          />
        )}
      </main>
    </div>
  )
}

export default AppLayout
