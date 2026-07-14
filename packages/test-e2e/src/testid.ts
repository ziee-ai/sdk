/**
 * i18n-safe semantic/component selectors for Playwright specs.
 *
 * Prefer `data-testid` over `getByText` / `getByRole({ name })`: visible text and
 * accessible names change under translation; a testid does not. The app owns its
 * typed testid UNION (a generated registry, e.g. `@ziee/kit/testIds.generated`);
 * this package supplies the selector HELPERS, generic over any id string, plus a
 * typed factory an app binds to its own union for compile-time typo-checking.
 */
import type { Page, Locator } from '@playwright/test'

/** A known (autocompleted) testid OR any string (derived/template ids like `${id}-row-${k}`). */
export type TestIdLike<Known extends string = string> = Known | (string & {})

/**
 * Select by data-testid.
 *
 *   await byTestId(page, 'user-form-email').fill('a@b.c')   // page-scoped
 *   await byTestId(row, `user-row-${id}`).click()           // scoped + derived id
 */
export const byTestId = (scope: Page | Locator, id: TestIdLike): Locator =>
  scope.getByTestId(id)

/**
 * Bind a typed `byTestId` to the app's generated testid union so an unknown id is
 * a COMPILE error (derived/template strings still accepted via `TestIdLike`).
 *
 *   import type { KnownTestId } from '@ziee/kit/testIds.generated'
 *   export const byTestId = makeByTestId<KnownTestId>()
 */
export const makeByTestId =
  <Known extends string>() =>
  (scope: Page | Locator, id: TestIdLike<Known>): Locator =>
    scope.getByTestId(id)

// ── Semantic selectors (role/label/text) — the accessibility-first ladder ────
// Prefer, in order: role → label → text → testid. These thin wrappers keep spec
// call-sites uniform; drop to `page.getByRole(...)` directly for advanced opts.

/** By ARIA role (+ optional accessible name). The top of the selector ladder. */
export const byRole = (
  scope: Page | Locator,
  role: Parameters<Page['getByRole']>[0],
  options?: Parameters<Page['getByRole']>[1],
): Locator => scope.getByRole(role, options)

/** By associated form-control label. */
export const byLabel = (
  scope: Page | Locator,
  text: string | RegExp,
  options?: { exact?: boolean },
): Locator => scope.getByLabel(text, options)

/** By visible text — last resort before testid (breaks under i18n). */
export const byText = (
  scope: Page | Locator,
  text: string | RegExp,
  options?: { exact?: boolean },
): Locator => scope.getByText(text, options)
