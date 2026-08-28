// Fixture: a LAYOUT module whose default export wraps route content via
// `children` — the shape `LayoutDefinition.component` declares, loaded through a
// real `() => import(...)` so the lazy path is exercised end to end.
import type { ReactNode } from 'react'

export default function SiteLayoutFixture({ children }: { children: ReactNode }) {
  return <div data-testid="site-shell">SITE-SHELL[{children}]</div>
}
