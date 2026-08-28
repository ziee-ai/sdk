// Fixture: a SECOND lazy layout. Two of them are the whole point — one lazy
// layout could never expose the key collision.
import type { ReactNode } from 'react'

export default function ReaderLayoutFixture({ children }: { children: ReactNode }) {
  return <div data-testid="reader-shell">READER-SHELL[{children}]</div>
}
