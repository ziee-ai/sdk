// Fixture for the lazy-loader detection suite: a module whose DEFAULT export is a
// component, loaded through a real `() => import('./LazyPage.tsx')` — the exact shape a
// module's `routes`/`slots` entry uses.
export default function LazyPage() {
  return <div data-testid="lazy-page">LAZY PAGE CONTENT</div>
}
