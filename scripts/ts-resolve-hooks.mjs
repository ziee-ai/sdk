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
import { existsSync, statSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, resolve as presolve } from 'node:path'

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
