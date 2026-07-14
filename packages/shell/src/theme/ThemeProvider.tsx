import { useEffect } from 'react'
import { useUpdate } from 'react-use'
import { Stores } from '@ziee/framework/stores'
import type { StoreProxy } from '@ziee/framework/stores'
import { Toaster, DialogHost } from '@ziee/kit'
import { ThemeContext, type ThemePreference } from './useTheme'
import { resolveSystemTheme } from './resolveTheme'
import { applyAccent, type AccentPreset } from './accentPresets'

interface ThemeProviderProps {
  children: React.ReactNode
}

/**
 * SEAM: the theme preference + accent live in the app-registered `ConfigClient`
 * store. `@ziee/shell` reads them through this typed view rather than importing
 * the app's config store — the runtime read is byte-identical to reading
 * `Stores.ConfigClient` directly. A consuming app registers a store named
 * `ConfigClient` exposing at least these three fields.
 */
interface ThemeConfigView {
  themePreference: ThemePreference
  accentPreset: AccentPreset
  setThemePreference: (theme: ThemePreference) => void
}

function themeConfig(): StoreProxy<ThemeConfigView> {
  return (Stores as unknown as { ConfigClient: StoreProxy<ThemeConfigView> })
    .ConfigClient
}

/**
 * App theme provider. Persists the preference via the ConfigClient store,
 * resolves system mode, toggles the `dark`/`light` class on <html>
 * (Tailwind/shadcn read it), and mounts the kit's imperative <Toaster/> +
 * <DialogHost/> once so module-level `message`/`dialog` work app-wide.
 */
export function ThemeProvider({ children }: ThemeProviderProps) {
  const { themePreference: selectedTheme, accentPreset } = themeConfig()

  const resolvedTheme =
    selectedTheme === 'system' ? resolveSystemTheme() : selectedTheme
  const isDarkMode = resolvedTheme === 'dark'

  const update = useUpdate()

  // Re-render on OS scheme change while in `system` mode.
  useEffect(() => {
    const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
    const handleChange = () => update()
    mediaQuery.addEventListener('change', handleChange)
    return () => mediaQuery.removeEventListener('change', handleChange)
  }, [selectedTheme])

  // Toggle the root theme class + apply the accent for the resolved theme.
  useEffect(() => {
    const root = document.documentElement
    if (isDarkMode) {
      root.classList.add('dark')
      root.classList.remove('light')
    } else {
      root.classList.add('light')
      root.classList.remove('dark')
    }
    // Apply the user's brand accent for the resolved theme (overrides --primary/--ring).
    applyAccent(root, accentPreset, isDarkMode)
    // NOTE: <meta name="theme-color"> (the iOS status/nav bar tint) is owned by
    // the LAYOUTS via useMetaThemeColor — it must match the surface at the screen
    // edges, which differs per layout (--card in the app shell, --background on
    // the blank/login layout). The pre-paint script in index.html sets a valid
    // (hex) initial value to avoid a flash; oklch is NOT valid for theme-color.
  }, [isDarkMode, accentPreset])

  return (
    <ThemeContext.Provider
      value={{
        selectedTheme,
        setTheme: themeConfig().setThemePreference,
        isDarkMode,
        resolvedTheme,
      }}
    >
      {children}
      <Toaster />
      <DialogHost />
    </ThemeContext.Provider>
  )
}
