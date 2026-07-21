import { appLayoutSeam } from '../app-store-seams'

/**
 * SEAM: reads `Stores.AppLayout.isSidebarCollapsed` through a typed view (never
 * the app's concrete store type); the cast preserves the live proxy so the read
 * stays reactive. Mirrors the AppLayout / DivScrollY seams.
 */
interface InsetSeam {
  isSidebarCollapsed: boolean
}

/**
 * The header's LEFT padding (px) that reserves space for the fixed
 * `SidebarToggleButton` (the sidebar collapse/expand button, which floats over
 * the top-left when the sidebar is collapsed). SINGLE SOURCE OF TRUTH for the
 * app header (`HeaderBarContainer`) and the split view's leftmost per-pane header
 * (ITEM-71) — so the two never drift.
 *
 * Web / non-Tauri: 48 when collapsed (clears the toggle), 12 otherwise. The
 * app-side `.desktop` override adds the macOS traffic-light clearance (118).
 */
export function useHeaderLeftInset(): number {
  const { isSidebarCollapsed } = (
    { AppLayout: appLayoutSeam.get() as InsetSeam }
  ).AppLayout
  return isSidebarCollapsed ? 48 : 12
}
