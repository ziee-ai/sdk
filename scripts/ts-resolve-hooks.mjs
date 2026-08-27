/**
 * Minimal `node --test` module resolver for the SDK packages' TypeScript sources.
 *
 * The packages are consumed as SOURCE (every `package.json` points `main`/`exports` at
 * `src/**.ts`) and are written for a bundler: `moduleResolution: "bundler"` lets them
 * import `./config` (extensionless) and `./module-system` (a directory holding an
 * `index.ts`). `node --test` has no bundler, so without this hook those specifiers throw
 * `ERR_MODULE_NOT_FOUND` / `ERR_UNSUPPORTED_DIR_IMPORT` and the suite dies at import time
 * — which is precisely how 3 of `@ziee/framework`'s 7 `node:test` files behaved the first
 * time anything ran them.
 *
 * Mirrors `src-app/ui/scripts/ts-resolve-hooks.mjs` in the consuming apps, minus their
 * app-specific `@/*` alias (the SDK packages resolve each other through real npm
 * workspace links, so only relative specifiers need help).
 */
import { existsSync, statSync, readFileSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, resolve as presolve } from 'node:path'
import { createRequire } from 'node:module'

const isFile = p => existsSync(p) && statSync(p).isFile()

/** `./foo` -> `./foo.ts` | `./foo.tsx` | `./foo/index.ts` | `./foo/index.tsx` */
const candidates = base => [base, `${base}.ts`, `${base}.tsx`, `${base}/index.ts`, `${base}/index.tsx`]

export async function resolve(spec, ctx, next) {
  if ((spec.startsWith('./') || spec.startsWith('../')) && ctx.parentURL?.startsWith('file:')) {
    const base = presolve(dirname(fileURLToPath(ctx.parentURL)), spec)
    for (const c of candidates(base)) {
      if (isFile(c)) return { url: pathToFileURL(c).href, shortCircuit: true }
    }
  }
  return next(spec, ctx)
}

/**
 * `.tsx` half of the hook.
 *
 * `node --experimental-strip-types` erases TYPE syntax; it does not understand JSX. So
 * until now NOTHING under `node --test` could import a `.tsx` source, which quietly
 * fenced off every component in the SDK — `LazyRouteRenderer`, `LazyComponentRenderer`,
 * `slots.tsx`, `RouterComponent` — from the only test runner the packages can actually
 * run (vitest is referenced by the `test` scripts but declared by no package; it is
 * supplied incidentally by whichever app hoists it, and a consumer that does not have it
 * gets `Cannot find package 'vitest'`). A renderer bug was therefore only reachable by a
 * consuming app's e2e, which is how the lazy-loader misdetection shipped.
 *
 * The transform uses the `typescript` compiler each package already declares as a
 * devDependency — no new dependency, no bundler. Types are erased by `transpileModule`
 * for `.tsx`, so the file is handed to Node as plain ESM and Node's own stripper never
 * sees it.
 */
const require = createRequire(import.meta.url)
let ts = null

export async function load(url, ctx, next) {
  if (!url.startsWith('file:') || !url.endsWith('.tsx')) return next(url, ctx)
  if (!ts) ts = require('typescript')
  const source = readFileSync(fileURLToPath(url), 'utf8')
  const { outputText } = ts.transpileModule(source, {
    fileName: fileURLToPath(url),
    compilerOptions: {
      target: ts.ScriptTarget.ESNext,
      module: ts.ModuleKind.ESNext,
      jsx: ts.JsxEmit.ReactJSX,
      isolatedModules: true,
      verbatimModuleSyntax: false,
    },
  })
  return { format: 'module', source: outputText, shortCircuit: true }
}
