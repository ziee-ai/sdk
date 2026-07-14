/**
 * Shared syncpack policy for ziee-SDK monorepos: keep every shared dependency on a
 * single version across all workspaces, pin `typescript` to `~` (its minors can break),
 * and `^` everything else. An app composes it with `defineSyncpack`, supplying its own
 * `source` globs and any app-specific `versionGroups` (e.g. desktop-only plugins).
 *
 *   // .syncpackrc.mjs
 *   import { defineSyncpack } from '@ziee/config/syncpack'
 *   export default defineSyncpack({
 *     source: ['package.json', 'apps/* /package.json'],
 *     versionGroups: [ ... app-specific exceptions ... ],
 *   })
 */

/** semver-range policy: `~` for typescript, `^` for everything else. */
export const semverGroups = [
  {
    label:
      'TypeScript minors can have breaking changes — pin to ~ across all workspaces.',
    range: '~',
    dependencies: ['typescript'],
    dependencyTypes: ['dev', 'prod'],
    packages: ['**'],
  },
  {
    label: 'Use caret ranges (^) for everything else.',
    range: '^',
    dependencies: ['**'],
    dependencyTypes: ['dev', 'prod'],
    packages: ['**'],
  },
]

/** The catch-all "same version everywhere it appears" group — MUST be last. */
export const sameRangeVersionGroup = {
  label: 'Everything else: same version everywhere it appears',
  dependencies: ['**'],
  packages: ['**'],
  policy: 'sameRange',
}

/**
 * Compose a full syncpack config. App-specific `versionGroups` (exceptions) are placed
 * BEFORE the catch-all `sameRangeVersionGroup` so they take precedence (syncpack matches
 * the first group a dependency falls into).
 */
export function defineSyncpack({ source = [], versionGroups = [], ...rest } = {}) {
  return {
    $schema: 'https://unpkg.com/syncpack@13/dist/schema.json',
    source,
    versionGroups: [...versionGroups, sameRangeVersionGroup],
    semverGroups,
    ...rest,
  }
}
