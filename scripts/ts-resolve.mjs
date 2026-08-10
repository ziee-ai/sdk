/**
 * `--import` entry point that registers `ts-resolve-hooks.mjs` (see that file for why
 * the SDK's TypeScript sources need a resolver under `node --test`).
 */
import { register } from 'node:module'
register('./ts-resolve-hooks.mjs', import.meta.url)
