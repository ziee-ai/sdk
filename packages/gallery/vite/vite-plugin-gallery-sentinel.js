/**
 * Worktree SENTINEL middleware (audit §7 "no-foreign-reuse").
 *
 * A gate:ui / visual / dev run must be able to prove that a gallery dev server
 * already listening on a port belongs to THIS worktree before reusing it —
 * otherwise (the fixed-1420 bug) a run silently reuses a SIBLING worktree's
 * server and tests the WRONG source tree. This dev-only middleware serves the
 * server's own worktree root at `/__worktree` as JSON; `gate-ui.mjs`'s reuse
 * branch (via `run-key.mjs::fetchSentinelRoot` + `serverIsThisWorktree`) and the
 * `prove-worktree-isolation.sh` harness read it to assert provenance.
 *
 * It is NOT a React gallery surface — no state-matrix / gallery-coverage impact.
 * `apply: 'serve'` so it never ships in a prod bundle.
 */
import { execSync } from 'node:child_process'

function resolveRoot(cwd) {
  try {
    const top = execSync('git rev-parse --show-toplevel', {
      cwd,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    }).trim()
    if (top) return top
  } catch {
    /* not a git tree / git absent */
  }
  // Fall back: strip from /src-app on (matches run-key.mjs::worktreeRoot).
  for (const m of ['/src-app', '\\src-app']) {
    const i = cwd.indexOf(m)
    if (i !== -1) return cwd.slice(0, i)
  }
  return cwd
}

export function gallerySentinelPlugin() {
  // Resolve once at config time — the server's cwd is stable for its lifetime.
  let root = process.cwd()
  const middleware = (req, res, next) => {
    if (req.url && (req.url === '/__worktree' || req.url.startsWith('/__worktree?'))) {
      res.setHeader('Content-Type', 'application/json')
      res.setHeader('Cache-Control', 'no-store')
      res.end(JSON.stringify({ worktreeRoot: root, pid: process.pid }))
      return
    }
    next()
  }
  return {
    name: 'gallery-sentinel',
    apply: 'serve',
    configResolved(cfg) {
      root = resolveRoot(cfg.root || process.cwd())
    },
    configureServer(server) {
      server.middlewares.use(middleware)
    },
    configurePreviewServer(server) {
      server.middlewares.use(middleware)
    },
  }
}
