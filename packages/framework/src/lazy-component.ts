import { isValidElement } from 'react'
import type { ComponentType } from 'react'

// ============================================================================
// How a "component-like" value is classified before it is rendered.
//
// Modules hand the shell three different things under one field name — an
// already-built element, a component, or a dynamic-import LOADER
// (`() => import('./Page')`) — and the renderers (`router/LazyRouteRenderer`,
// `@ziee/shell`'s `LazyComponentRenderer`) have to tell them apart to know
// whether a `<Suspense>` boundary is required.
//
// ── The bug this module exists to end ───────────────────────────────────────
// Both renderers used to answer "is this a loader?" with, among other things,
// `!component.name` — a loader was assumed to be ANONYMOUS. That holds for
// `const P = lazyWithPreload(...)`-style helpers and for a loader passed as a
// bare argument, and it is false for the shape the docs actually show:
//
//     { component: () => import('./Page') }   // .name === 'component'
//
// JS infers a function's name from the binding it initializes, so an
// object-property initializer is NOT anonymous. Such an entry was classified as
// a plain component, invoked, and returned a Promise; React suspended on it,
// and because a fresh promise is created on every call the suspension never
// settles. The nearest boundary is the APP's, not the renderer's, so the region
// rendered the app fallback forever — no exception, no console output, nothing
// in the network tab that looked wrong. A silently empty region.
//
// Widening the name pattern would only move the goalposts to the next spelling
// (`{ element: () => import(...) }`, `{ Page: () => import(...) }`, a named
// `loadPage` helper). So classification is by SHAPE and by an explicit MARKER,
// and the residual ambiguity is made LOUD instead of silent:
//
//   1. `isValidElement`            — an element, structurally.
//   2. `$$typeof === react.lazy`   — a `React.lazy` exotic, structurally.
//   3. `markLazyLoader(fn)`        — an explicit, unambiguous opt-in marker.
//   4. a dynamic `import(` in the function's own source — `import()` is
//      SYNTAX: a bundler rewrites the specifier and a minifier renames every
//      identifier around it, but neither can rename the operator itself, so
//      this survives production builds where a name-based guess does not.
//   5. anonymous 0-arg function    — the pre-existing heuristic, kept ONLY as a
//      trailing fallback so `lazyWithPreload`-style loaders keep working. It is
//      no longer load-bearing.
//
// Anything still classified `component` that is a plain 0-arg function stays
// ambiguous by construction (a loader built by a helper the source of which
// shows no `import(`). That case is handled at render time by the tripwire
// below: if such a function returns a thenable, the renderer says so, by name,
// instead of handing React a promise it can never settle.
// ============================================================================

/** A dynamic-import loader: `() => import('./Page')`. */
export type LazyLoader = () => Promise<{ default: ComponentType<any> }>

/** Brand applied by {@link markLazyLoader}; `Symbol.for` so duplicate copies of
 *  this module (two bundles, a dev/prod split) still agree. */
const LAZY_LOADER_MARK = Symbol.for('ziee.framework.lazyLoader')

/** React's own tag for a `React.lazy(...)` exotic component. */
const REACT_LAZY = Symbol.for('react.lazy')

/** `React.memo(...)` — an EXOTIC OBJECT, not a function. It holds the wrapped
 *  type in `.type`, which may itself be a lazy exotic (`memo(lazy(...))`). */
const REACT_MEMO = Symbol.for('react.memo')

/**
 * `import(` as SYNTAX. Deliberately not anchored to a specific bundler's helper
 * (`__vitePreload(() => import("./x-a1b2.js"))` matches just as well as the
 * authored form), because the operator is the one token no build step may
 * rewrite.
 */
const DYNAMIC_IMPORT = /\bimport\s*\(/

/**
 * Mark a function as a dynamic-import loader. The unambiguous way to declare an
 * entry whose shape cannot be recognised — a loader produced by a helper, or one
 * that resolves an already-imported module:
 *
 * ```ts
 * { component: markLazyLoader(() => loadPageSomehow()) }
 * ```
 *
 * Returns the same function, so it wraps an entry in place.
 */
export function markLazyLoader<T extends LazyLoader>(loader: T): T {
  Object.defineProperty(loader, LAZY_LOADER_MARK, {
    value: true,
    enumerable: false,
    configurable: true,
  })
  return loader
}

/** Whether `value` carries the {@link markLazyLoader} brand. */
export function isMarkedLazyLoader(value: unknown): boolean {
  return (
    typeof value === 'function' &&
    (value as unknown as Record<symbol, unknown>)[LAZY_LOADER_MARK] === true
  )
}

/** What a component-like value turned out to be. */
export type ComponentKind =
  /** An already-built React element — render as-is. */
  | 'element'
  /** A `React.lazy(...)` exotic — render inside a `<Suspense>`. */
  | 'react-lazy'
  /** A dynamic-import loader — wrap with `lazy()` inside a `<Suspense>`. */
  | 'loader'
  /** A plain component (function or class) — render directly. */
  | 'component'
  /** Nothing renderable — the caller must say so out loud. */
  | 'invalid'

/** The classification plus the rule that decided it (for diagnostics). */
export interface Classification {
  kind: ComponentKind
  /** Which rule fired — quoted verbatim in the dev diagnostics. */
  reason: string
  /**
   * True when the value was called `component` but a 0-arg plain function could
   * equally have been a loader built by a helper. The renderers arm the
   * runtime tripwire for these, and only these.
   */
  ambiguous: boolean
}

export function classifyComponentLike(value: unknown): Classification {
  if (isValidElement(value)) {
    return { kind: 'element', reason: 'isValidElement', ambiguous: false }
  }

  // ── React EXOTIC components are objects, not functions ────────────────────
  // `React.memo(...)`, `React.forwardRef(...)`, `React.lazy(...)`, a context
  // Provider — every one of them is a plain object carrying a `$$typeof`
  // symbol, and every one of them is a legitimate element TYPE that React
  // renders. Falling through to the `typeof value !== 'function'` arm below
  // would classify all of them `invalid` and render NOTHING — reintroducing,
  // for exotics, the exact silent-empty-region failure this module exists to
  // end. (The pre-fix renderers got these right by accident: they only special-
  // cased functions and handed everything else straight to `createElement`.)
  const exoticTag =
    value !== null && typeof value === 'object'
      ? (value as { $$typeof?: symbol }).$$typeof
      : undefined

  if (typeof exoticTag === 'symbol') {
    if (exoticTag === REACT_LAZY) {
      return { kind: 'react-lazy', reason: '$$typeof === react.lazy', ambiguous: false }
    }
    // `memo(lazy(...))` still suspends, so it still needs the boundary; the
    // wrapped type is reachable through `.type`.
    if (
      exoticTag === REACT_MEMO &&
      (value as { type?: { $$typeof?: symbol } }).type?.$$typeof === REACT_LAZY
    ) {
      return { kind: 'react-lazy', reason: '$$typeof === react.memo wrapping react.lazy', ambiguous: false }
    }
    return {
      kind: 'component',
      reason: `React exotic component ($$typeof === ${String(exoticTag.description ?? exoticTag)})`,
      ambiguous: false,
    }
  }

  if (typeof value !== 'function') {
    return {
      kind: 'invalid',
      reason: `not a function, element, or React component type (got ${value === null ? 'null' : typeof value})`,
      ambiguous: false,
    }
  }

  const fn = value as ComponentType<any> & { prototype?: { isReactComponent?: unknown } }

  // A class component is a function too — and calling one as a loader would
  // throw — so it is settled before any loader rule runs.
  if (fn.prototype?.isReactComponent) {
    return { kind: 'component', reason: 'class component', ambiguous: false }
  }

  if (isMarkedLazyLoader(fn)) {
    return { kind: 'loader', reason: 'markLazyLoader()', ambiguous: false }
  }

  const zeroArg = (fn as (...a: any[]) => unknown).length === 0

  // A component that declares `props` takes an argument; a loader never does.
  // Everything below therefore only applies to 0-arg functions.
  if (zeroArg) {
    let source = ''
    try {
      source = Function.prototype.toString.call(fn)
    } catch {
      // A Proxy or a host function may refuse `toString`; fall through to the
      // name fallback rather than throwing out of a render.
      source = ''
    }
    if (DYNAMIC_IMPORT.test(source)) {
      return { kind: 'loader', reason: 'dynamic import() in source', ambiguous: false }
    }
    if (!fn.name) {
      return { kind: 'loader', reason: 'anonymous 0-arg function (legacy fallback)', ambiguous: false }
    }
    return {
      kind: 'component',
      reason: `named 0-arg function '${fn.name}' with no import() in its source`,
      ambiguous: true,
    }
  }

  return { kind: 'component', reason: 'function taking props', ambiguous: false }
}

/** `true` outside a production build. Guarded: `process` is absent in some
 *  browser bundles, and bundlers const-fold the whole check away in prod. */
export function isDevBuild(): boolean {
  try {
    return typeof process === 'undefined' || process.env?.NODE_ENV !== 'production'
  } catch {
    return true
  }
}

const warned = new Set<string>()

/** Log once per (id, message) pair — a renderer runs on every render. */
function warnOnce(key: string, ...args: unknown[]): void {
  if (warned.has(key)) return
  warned.add(key)
  console.error(...(args as [unknown]))
}

/** Test seam: forget which diagnostics have already been emitted. */
export function __resetLazyComponentWarnings(): void {
  warned.clear()
}

/**
 * A value that cannot be rendered at all. Previously this produced
 * `<undefined />` / a React "type is invalid" crash with no hint of WHICH slot
 * or route was at fault; now the id is in the message and the region renders
 * nothing rather than taking the tree down.
 */
export function warnUnrenderable(debugId: string | undefined, reason: string): void {
  if (!isDevBuild()) return
  warnOnce(
    `invalid:${debugId ?? '?'}:${reason}`,
    `[ziee] ${describe(debugId)} was given a value that is not renderable (${reason}). ` +
      'Nothing will render here. Expected a React element, a component, or a ' +
      "dynamic-import loader such as `() => import('./Page')`.",
  )
}

/**
 * The residual case: a NAMED 0-arg function that showed no `import(` in its
 * source, rendered as a component, that turned out to return a thenable. That
 * is a loader in disguise — the exact failure that used to be silent, because
 * React suspends on the returned promise and, since a fresh promise is produced
 * on every call, the enclosing boundary never settles.
 */
export function warnLoaderRenderedAsComponent(
  debugId: string | undefined,
  fnName: string,
): void {
  if (!isDevBuild()) return
  warnOnce(
    `thenable:${debugId ?? '?'}:${fnName}`,
    `[ziee] ${describe(debugId)} rendered '${fnName}' as a component, but it returned a ` +
      'Promise — it is a dynamic-import loader that this renderer could not recognise ' +
      'from its shape. Nothing will render here. Wrap it: ' +
      "`markLazyLoader(() => import('./Page'))` from '@ziee/framework'.",
  )
}

function describe(debugId: string | undefined): string {
  return debugId ? `'${debugId}'` : 'an unnamed slot/route entry'
}

/** Duck-typed thenable check for the tripwire (a component's return value). */
export function isThenable(value: unknown): value is PromiseLike<unknown> {
  return (
    value !== null &&
    (typeof value === 'object' || typeof value === 'function') &&
    typeof (value as { then?: unknown }).then === 'function'
  )
}
