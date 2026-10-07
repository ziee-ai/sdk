/**
 * Path-parameter substitution shared by BOTH ApiClient transports.
 *
 * The browser transport (`core.ts::performCall`) and the app's SSR transport
 * (`@comic/ui`'s `src/core/ssr/apiClientSync.ts::ssrFetch`) each substituted
 * `{capture}` path parameters with their OWN loop, and the two disagreed on a
 * `null` capture: the SDK gated only on `!== undefined` (a `null` became the
 * literal path segment "null" — a silent request for a resource that cannot
 * exist), while the SSR transport threw. That divergence is phibya/comic#502.
 *
 * This is the ONE implementation both call. Semantics (pinned by both sides'
 * tests, and by the app's `path-substitution-parity.test.ts`):
 *
 *  - a capture whose value is `undefined` OR `null` is a programming error and
 *    THROWS `Missing required parameter: <name>` — never a "null" path segment;
 *  - `''` is a valid value and substitutes as the empty segment (existing
 *    behaviour on both sides, kept);
 *  - every other value is substituted with `String(value)` — numbers are
 *    stringified (existing behaviour);
 *  - NO URL-encoding is applied to substituted values here (neither side
 *    encoded before this change; see the #502 report's follow-up note);
 *  - the returned capture names are in TEMPLATE order and are the raw
 *    `{...}` bodies (untrimmed), exactly what the callers' GET-query / body
 *    exclusion checks already keyed on, so those checks cannot drift with the
 *    substitution.
 *
 * Purely functional: no imports, no async, no DOM, no `FormData` coercion —
 * which is what lets the SSR bundle (QuickJS-ng, synchronous, no module
 * loader) import this the same way the browser bundle does.
 */

export interface SubstitutedPath {
  /** The template with every `{capture}` replaced by its value. */
  path: string
  /** The capture names found in the template, in template order. */
  captures: string[]
}

/**
 * Find the `{capture}` names in an endpoint path template and substitute them
 * from `params`.
 *
 * @param pathTemplate the endpoint path template, e.g. `/api/series/{slug}`
 * @param params the values object (or a plain map of capture name → value;
 *   the FormData branch of `performCall` builds such a map so a File payload
 *   is never copied)
 * @throws {Error} `Missing required parameter: <name>` when a capture's value
 *   is `undefined` (key absent) or `null`
 */
export function substitutePathParams(
  pathTemplate: string,
  params: unknown,
): SubstitutedPath {
  const captures = (pathTemplate.match(/{([^}]+)}/g) || []).map(match =>
    match.slice(1, -1),
  )
  const p = (params ?? {}) as Record<string, unknown>
  let path = pathTemplate
  for (const capture of captures) {
    const key = capture.trim()
    const value = p[key]
    if (value === undefined || value === null) {
      throw new Error(`Missing required parameter: ${key}`)
    }
    path = path.replace(`{${capture}}`, String(value))
  }
  return { path, captures }
}
