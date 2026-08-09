import { appLayoutChrome } from '../app-store-seams'

/**
 * SEAM: reads `Stores.AppLayout.isSidebarCollapsed` through a typed view (never
 * the app's concrete store type); the cast preserves the live proxy so the read
 * stays reactive. Mirrors the AppLayout / DivScrollY seams.
 */
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
  // Optional read: a header can render in a store-LESS context (an isolated
  // gallery overlay, a layout-less route) where the AppLayout seam is absent.
  // Fall back to the not-collapsed inset rather than throwing.
  const { isSidebarCollapsed } = appLayoutChrome()
  return isSidebarCollapsed ? 48 : 12
}
