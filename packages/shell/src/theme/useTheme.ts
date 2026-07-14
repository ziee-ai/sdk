import { createContext, useContext } from 'react'

export type ThemeName = 'light' | 'dark'

/**
 * The persisted theme preference. Framework-level so `@ziee/shell` doesn't
 * depend on the app's config store. A consuming app's own `ThemePreference`
 * (this exact literal union) is structurally identical, so the seam
 * (`Stores.ConfigClient`) type-checks either way.
 */
export type ThemePreference = 'light' | 'dark' | 'system'

export interface ThemeContextValue {
  selectedTheme: ThemePreference
  resolvedTheme: ThemeName
  isDarkMode: boolean
  setTheme: (theme: ThemePreference) => void
}

export const ThemeContext = createContext<ThemeContextValue | undefined>(
  undefined,
)

export function useTheme() {
  const context = useContext(ThemeContext)
  if (!context) {
    throw new Error('useTheme must be used within ThemeProvider')
  }
  return context
}
