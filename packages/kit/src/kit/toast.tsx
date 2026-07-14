import * as React from 'react'
import { toast as sonnerToast } from 'sonner'
import { Toaster as BaseToaster } from '../shadcn/sonner'
import { useThemeOptional } from './theme'

// Toaster wired to the kit ThemeProvider (the vendored Sonner wrapper resolves theme from a
// provider we don't mount, so it would ignore the app's light/dark choice). Mount ONCE at root.
export function Toaster(props: React.ComponentProps<typeof BaseToaster>) {
  const theme = useThemeOptional()?.resolvedTheme
  return <BaseToaster theme={theme} {...props} />
}

// Imperative toast API. Mount <Toaster /> once at the app root.
// toast.success('Saved') etc. Returns the toast id (dismiss via toast.dismiss(id)).
// NOTE: `duration` is MILLISECONDS (sonner-native).
type Opts = { description?: string; duration?: number }
export const message = {
  success: (content: string, o?: Opts) => sonnerToast.success(content, o),
  error: (content: string, o?: Opts) => sonnerToast.error(content, o),
  info: (content: string, o?: Opts) => sonnerToast.info(content, o),
  warning: (content: string, o?: Opts) => sonnerToast.warning(content, o),
  loading: (content: string, o?: Opts) => sonnerToast.loading(content, o),
  dismiss: (id?: string | number) => sonnerToast.dismiss(id),
}
export { sonnerToast as toast }
