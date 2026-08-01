// @ziee/shell — the reusable, slot-driven application SHELL.
//
// Ships the generic, app-agnostic shell infrastructure an app fills instead of
// copying verbatim: the theme provider + tokens, the app-bootstrap body
// (`AppShell`), the error boundary, the universal lazy-component renderer, and
// the chromeless blank layout. Hard-codes NO pages or modules — an app supplies
// those through the module system (@ziee/framework) + its slots, and supplies
// its branding/nav.
//
// SEAMS (app-registered stores the shell reads through typed views, never the
// app's concrete store types):
//   - `Stores.ConfigClient` — `{ themePreference, accentPreset, setThemePreference }`
//     (ThemeProvider).
//   - `Stores.AppLayout` — `{ nativeScroll? }` (DivScrollY; optional).
//   - `Stores.Auth` — the sync lifecycle (via `initSync`, passed to AppShell).
//
// Permission-gating primitives (`Can` / `usePermission` / `evaluatePermission`)
// live in `@ziee/framework/permissions`; routing lives in
// `@ziee/framework/router`. `@ziee/shell` composes on top of both.

// Theme
export { ThemeProvider } from './theme/ThemeProvider'
export { resolveSystemTheme } from './theme/resolveTheme'
export {
  ThemeContext,
  useTheme,
  type ThemeName,
  type ThemePreference,
  type ThemeContextValue,
} from './theme/useTheme'
export {
  toRgbHex,
  setMetaThemeColorFromVar,
  useMetaThemeColor,
} from './theme/themeColor'
export {
  ACCENT_PRESETS,
  DEFAULT_ACCENT,
  ACCENT_ORDER,
  applyAccent,
  type AccentPreset,
  type AccentVariant,
  type AccentPresetDef,
} from './theme/accentPresets'

// Error handling
export { AppErrorBoundary } from './error/AppErrorBoundary'

// Core render components
export { Loading, type LoadingProps } from './components/Loading'
export {
  LazyComponentRenderer,
  WidgetRenderer,
} from './components/LazyComponentRenderer'
export { DivScrollY, type DivScrollYProps } from './components/DivScrollY'
export { Drawer, type DrawerProps } from './components/Drawer'
export { ResizeHandle } from './components/ResizeHandle'
export { HeaderBarContainer } from './components/HeaderBarContainer'
export { useHeaderLeftInset } from './hooks/useHeaderLeftInset'

// Layouts
export { BlankLayout } from './layouts/BlankLayout'
export { AppLayout, type AppLayoutProps } from './layouts/AppLayout'
export type {
  SidebarNavItem,
  SidebarToolItem,
  SidebarActionItem,
  SidebarWidgetItem,
} from './layouts/appLayoutSlots'

// Settings scaffold (generic; the platform-variant `SettingsPage` stays
// app-side behind the `@/` override seam)
export { SettingsPageContainer } from './settings/SettingsPageContainer'

// Bootstrap
export { AppShell, type AppShellProps } from './bootstrap/AppShell'

// Hooks
export { usePrefetchModules } from './hooks/usePrefetchModules'
export {
  useWindowMinSize,
  useElementMinSize,
  calculateMinSize,
  applyHysteresis,
  breakpointValues,
  type Breakpoint,
  type MinSize,
} from './hooks/useWindowMinSize'
export { appLayoutSeam, configClientSeam } from './app-store-seams'
