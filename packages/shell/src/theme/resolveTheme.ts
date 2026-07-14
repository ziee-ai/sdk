export const resolveSystemTheme = (): 'light' | 'dark' => {
  const mediaQuery = window.matchMedia('(prefers-color-scheme: dark)')
  return mediaQuery.matches ? 'dark' : 'light'
}
