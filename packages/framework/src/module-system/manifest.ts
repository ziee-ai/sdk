import type { AppModule, ModuleLoadContext } from './types'

/**
 * One entry of the build-generated module manifest. The build plugin
 * (`vite-plugin-module-manifest`) emits an array of these into a virtual module
 * baked in the ENTRY chunk: the cheap decision layer (`name` / `shouldLoad` /
 * `routePaths` / `dependencies`) is eager, while `load()` is a dynamic import of
 * the heavy module body, pulled ONLY once the module is selected to register.
 */
export interface ModuleManifestEntry {
  /** `metadata.name` — the stable module id (matches the runtime module). */
  name: string
  /**
   * The lifted `shouldLoad` predicate (or undefined ⇒ CORE, always loaded).
   * Evaluated against a {@link ModuleLoadContext}; references only `ctx` + the
   * whitelisted `Permissions` enum (enforced at build time).
   */
  shouldLoad?: (ctx: ModuleLoadContext) => boolean
  /** The `path:` strings from the module's `routes` (for route-driven loading). */
  routePaths: string[]
  /** `dependencies` — module names that must register before this one. */
  dependencies: string[]
  /** Dynamic import of the full module body (`export default createModule(...)`). */
  load: () => Promise<{ default: AppModule }>
}

/** True if the entry is CORE (no predicate) or its predicate passes for `ctx`. */
export function isEligible(
  entry: ModuleManifestEntry,
  ctx: ModuleLoadContext,
): boolean {
  if (!entry.shouldLoad) return true
  try {
    return entry.shouldLoad(ctx)
  } catch {
    // A throwing predicate must never wedge the loader — treat as not-eligible;
    // it re-evaluates on the next ctx change.
    return false
  }
}

/** CORE entries only (no `shouldLoad`) — the always-load boot set. */
export function coreEntries(
  manifest: ModuleManifestEntry[],
): ModuleManifestEntry[] {
  return manifest.filter(e => !e.shouldLoad)
}

/**
 * Topologically order a set of entries by `dependencies` (Kahn/DFS, matching the
 * existing loader). A dependency not present in `available` is skipped with a
 * warning — the same lenient behavior the current loader has — so staged
 * registration of a subset never throws. Cycles throw (a real authoring bug).
 */
export function orderByDependencies(
  entries: ModuleManifestEntry[],
): ModuleManifestEntry[] {
  const byName = new Map(entries.map(e => [e.name, e]))
  const sorted: ModuleManifestEntry[] = []
  const visited = new Set<string>()
  const visiting = new Set<string>()

  const visit = (name: string, path: string[]): void => {
    if (visited.has(name)) return
    if (visiting.has(name)) {
      throw new Error(
        `Circular module dependency: ${[...path, name].join(' -> ')}`,
      )
    }
    const entry = byName.get(name)
    if (!entry) return // dep outside this batch — resolved in another wave
    visiting.add(name)
    for (const dep of entry.dependencies) {
      if (!byName.has(dep)) {
        console.warn(
          `[loader] module "${name}" depends on "${dep}" not in this load wave; ordering skipped for that edge`,
        )
        continue
      }
      visit(dep, [...path, name])
    }
    visiting.delete(name)
    visited.add(name)
    sorted.push(entry)
  }

  for (const e of entries) visit(e.name, [])
  return sorted
}

/**
 * Convert a route `path` pattern (`/chat/:conversationId`) to a matcher against
 * a concrete pathname. Used by the route-driven safety net to find the module
 * that owns a not-yet-registered path.
 */
function pathMatches(pattern: string, pathname: string): boolean {
  // Exact fast-path.
  if (pattern === pathname) return true
  const p = pattern.split('/').filter(Boolean)
  const a = pathname.split('/').filter(Boolean)
  // A trailing optional param (`:x?`) lets the pattern be one segment longer.
  if (a.length < p.length - 1 || a.length > p.length) return false
  for (let i = 0; i < p.length; i++) {
    const seg = p[i]
    if (seg.startsWith(':')) {
      if (a[i] === undefined && seg.endsWith('?')) continue
      if (a[i] === undefined) return false
      continue // param matches any single segment
    }
    if (seg !== a[i]) return false
  }
  return true
}

/** The manifest entry owning `pathname`, if any (route-driven loading). */
export function entryForPath(
  manifest: ModuleManifestEntry[],
  pathname: string,
): ModuleManifestEntry | undefined {
  return manifest.find(e => e.routePaths.some(p => pathMatches(p, pathname)))
}
