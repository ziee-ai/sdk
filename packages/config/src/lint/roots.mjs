/**
 * Shared arg-parsing for the design lints so each one is parameterized over the
 * app's source tree(s) instead of a hard-coded `../src` scan path.
 *
 * `--root=<dir>` / `--root <dir>` (repeatable) — a directory to scan; resolved
 * relative to the process CWD. When no `--root` is given, the caller's `defaults`
 * are used (also resolved vs CWD). This is what lets ANY app run the lint against
 * its own tree: `ziee-lint-colors --root=src --root=../desktop/ui/src`.
 */
import path from 'node:path'

export function parseRoots(argv = process.argv.slice(2), defaults = ['src']) {
  const roots = []
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a.startsWith('--root=')) roots.push(a.slice('--root='.length))
    else if (a === '--root' && argv[i + 1]) roots.push(argv[++i])
  }
  const chosen = roots.length ? roots : defaults
  return chosen.map(r => path.resolve(process.cwd(), r))
}

/** Read a repeatable `--<name>=<val>` / `--<name> <val>` flag; returns list. */
export function parseMulti(name, argv = process.argv.slice(2)) {
  const out = []
  const eq = `--${name}=`
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i]
    if (a.startsWith(eq)) out.push(a.slice(eq.length))
    else if (a === `--${name}` && argv[i + 1]) out.push(argv[++i])
  }
  return out
}

/** Read a single `--<name>=<val>` / `--<name> <val>` flag; returns string|undefined. */
export function parseOne(name, argv = process.argv.slice(2)) {
  return parseMulti(name, argv).at(-1)
}
