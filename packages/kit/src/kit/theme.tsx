import * as React from 'react'

// Theme provider for the shadcn world: there is NO runtime token/theme engine and NO JS token
// object. Colors live entirely in CSS variables under `:root` / `html.dark` (see index.css);
// this provider only decides WHICH set is active by toggling the `dark` class on <html>.
// Components read colors via Tailwind classes (bg-card, text-foreground, border, …).
export type ThemePreference = 'light' | 'dark' | 'system'
export type ResolvedTheme = 'light' | 'dark'

export interface ThemeContextValue {
  /** The user's choice (may be 'system'). */
  theme: ThemePreference
  /** The concrete theme in effect after resolving 'system'. */
  resolvedTheme: ResolvedTheme
  isDark: boolean
  setTheme: (theme: ThemePreference) => void
}

const ThemeContext = React.createContext<ThemeContextValue | undefined>(undefined)

export function useTheme(): ThemeContextValue {
  const ctx = React.useContext(ThemeContext)
  if (!ctx) throw new Error('useTheme must be used within <ThemeProvider>')
  return ctx
}

/** Non-throwing variant for infra that may render outside a provider (e.g. the Toaster). */
export function useThemeOptional(): ThemeContextValue | undefined {
  return React.useContext(ThemeContext)
}

function systemTheme(): ResolvedTheme {
  return typeof window !== 'undefined' && window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}

export interface ThemeProviderProps {
  children: React.ReactNode
  /** Controlled preference. Omit to let the provider own it (persisted to localStorage). */
  value?: ThemePreference
  /** Called when the preference changes (required in controlled mode). */
  onThemeChange?: (theme: ThemePreference) => void
  /** Initial preference when uncontrolled. */
  defaultTheme?: ThemePreference
  /** localStorage key when uncontrolled. */
  storageKey?: string
}

export function ThemeProvider({
  children, value, onThemeChange, defaultTheme = 'system', storageKey = 'ziee-theme',
}: ThemeProviderProps) {
  const controlled = value !== undefined
  const [internal, setInternal] = React.useState<ThemePreference>(() => {
    if (controlled) return value
    if (typeof window === 'undefined') return defaultTheme
    // storage access can throw (private mode / disabled cookies / sandboxed iframe).
    try { return (localStorage.getItem(storageKey) as ThemePreference | null) ?? defaultTheme } catch { return defaultTheme }
  })
  const theme = controlled ? value : internal

  // re-render on OS scheme change while in 'system' mode
  const [, force] = React.useReducer((n: number) => n + 1, 0)
  React.useEffect(() => {
    if (theme !== 'system' || typeof window === 'undefined') return
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    const onChange = () => force()
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [theme])

  const resolvedTheme: ResolvedTheme = theme === 'system' ? systemTheme() : theme
  const isDark = resolvedTheme === 'dark'

  // apply the class to <html> + sync the address-bar meta color from the live --background var.
  React.useEffect(() => {
    if (typeof document === 'undefined') return
    const root = document.documentElement
    // light is the `:root` baseline (no `html.light` rule) — only the `dark` class matters.
    root.classList.toggle('dark', isDark)
    const bg = getComputedStyle(root).getPropertyValue('--background').trim()
    if (bg) {
      let meta = document.querySelector('meta[name="theme-color"]')
      if (!meta) { meta = document.createElement('meta'); meta.setAttribute('name', 'theme-color'); document.head.appendChild(meta) }
      meta.setAttribute('content', /^(#|rgb|hsl|oklch)/.test(bg) ? bg : `hsl(${bg})`)
    }
  }, [isDark])

  const setTheme = React.useCallback((next: ThemePreference) => {
    if (!controlled) {
      setInternal(next)
      try { localStorage.setItem(storageKey, next) } catch { /* ignore */ }
    }
    onThemeChange?.(next)
  }, [controlled, onThemeChange, storageKey])

  const ctx = React.useMemo<ThemeContextValue>(
    () => ({ theme, resolvedTheme, isDark, setTheme }),
    [theme, resolvedTheme, isDark, setTheme],
  )
  return <ThemeContext.Provider value={ctx}>{children}</ThemeContext.Provider>
}
