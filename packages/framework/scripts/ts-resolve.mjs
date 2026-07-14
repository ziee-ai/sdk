// Minimal `node --test` resolver: maps extensionless RELATIVE specifiers
// (`./sse-types`, `../sync/connection`) to the real `.ts` / `.tsx` / index
// file, so the package's TS sources (which use extensionless imports, per
// `allowImportingTsExtensions`) load under `node --test --experimental-strip-types`
// with no bundler. Mirrors ziee ui's `scripts/node-test-hooks.mjs`.
import { register } from 'node:module'
register('./ts-resolve-hooks.mjs', import.meta.url)
