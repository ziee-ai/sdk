import React, { useLayoutEffect } from 'react'
import { useMetaThemeColor } from '../theme/themeColor'

interface BlankLayoutProps {
  children: React.ReactNode
}

/**
 * BlankLayout — a chromeless layout for auth / login / setup pages that render
 * outside the app shell. Supplies a `main` landmark, paints `--background` to
 * the screen edges, and matches the iOS status/nav bar to it.
 */
export function BlankLayout({ children }: BlankLayoutProps) {
  // Blank/login pages paint --background to the edges → match the iOS bars.
  useMetaThemeColor('--background')

  // useLayoutEffect (not useEffect) so the background color is applied AND
  // restored synchronously before the browser paints — with useEffect the
  // change lands a frame late, producing a visible white/blank flash on mount
  // and on teardown (e.g. closing a popup).
  useLayoutEffect(() => {
    // set root document background color based on theme, restore on teardown
    const root = document.documentElement
    const prev = root.style.backgroundColor
    root.style.backgroundColor = 'var(--background)'
    return () => {
      root.style.backgroundColor = prev
    }
  }, [])

  // Provide a top-level `main` landmark so assistive tech has a primary
  // content region on the auth/blank pages (these render outside the app
  // shell that normally supplies it). `display: contents` keeps the element
  // out of the box tree so it has zero layout impact while still exposing the
  // landmark role.
  return <main style={{ display: 'contents' }}>{children}</main>
}
