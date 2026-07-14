import { existsSync, statSync } from 'node:fs'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { dirname, resolve as presolve } from 'node:path'

const isFile = p => existsSync(p) && statSync(p).isFile()

// Resolve extensionless RELATIVE specifiers to their .ts/.tsx/index file.
export async function resolve(spec, ctx, next) {
  if ((spec.startsWith('./') || spec.startsWith('../')) && ctx.parentURL) {
    const base = presolve(dirname(fileURLToPath(ctx.parentURL)), spec)
    for (const c of [base, base + '.ts', base + '.tsx', base + '/index.ts', base + '/index.tsx']) {
      if (isFile(c)) return { url: pathToFileURL(c).href, shortCircuit: true }
    }
  }
  return next(spec, ctx)
}
