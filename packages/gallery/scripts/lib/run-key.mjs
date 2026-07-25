/**
 * Unified per-worktree RUN KEY — the Node twin of the Rust
 * `ziee-build-support::worktree_db` key (audit §7). ONE mechanism every
 * harness/dev/gallery resource derives its port-base / namespace from, so N
 * concurrent git worktrees never collide on a shared mutable resource.
 *
 * Byte-identical to Rust: `worktreeKey` uses the SAME FNV-1a
 * (0xcbf29ce484222325 / 0x100000001b3, fold `h ^ (h>>32)` → 8 hex) over the SAME
 * absolute worktree-root string, and `portBase` uses the SAME modulo math. A
 * shared fixture vector ("/data/pbya/ziee/tmp/xwt-a" → "12080097") is asserted in
 * BOTH this package's node test AND the Rust `#[cfg(test)]`, so a drift in either
 * language goes red.
 *
 * INVARIANT (audit §7): no fixed port anywhere — the key only seeds the SEARCH
 * START; the actual port is the first OS-bindable one from that base
 * (`pickBindablePort`, mirroring `port-manager.ts::isPortBindable`).
 */
import { execSync } from 'node:child_process'
import { createServer } from 'node:net'

/**
 * Disjoint port FLOORS (search starts) per harness, so no two harnesses share a
 * base even at the same key. Named table (never inline magic numbers) so floors
 * retune centrally. span 200 each. All below the ephemeral range's typical churn
 * and > the OS reserved range.
 */
export const PORT_FLOORS = {
  webGallery: { floor: 20000, span: 200 }, // web `npm run dev` / gate:ui / visual
  desktopGallery: { floor: 22000, span: 200 }, // desktop dev / gate:ui / gallery
  webE2eVite: { floor: 9000, span: 200 }, // web e2e vite (matches historical 9000)
  webE2eBackend: { floor: 9100, span: 200 }, // web e2e backend (historical 9100)
  webE2ePg: { floor: 54331, span: 100 }, // web e2e postgres (historical 54331)
  desktopE2eBackend: { floor: 9600, span: 200 }, // desktop e2e backend — OFF the web 9100 overlap
  desktopE2ePg: { floor: 54600, span: 100 }, // desktop e2e postgres — OFF the web 54331 overlap
}

/**
 * FNV-1a 64-bit → low-32 fold → 8 hex. BigInt to match Rust's u64 wrapping mul
 * exactly (JS Number loses precision past 2^53). `h ^ (h>>32)` truncated to u32
 * mirrors Rust's `(hash ^ (hash >> 32)) as u32`.
 */
export function fnv1a8(input) {
  let h = 0xcbf29ce484222325n
  for (const b of Buffer.from(input, 'utf8')) {
    h ^= BigInt(b)
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn
  }
  const folded = Number((h ^ (h >> 32n)) & 0xffffffffn) >>> 0
  return folded.toString(16).padStart(8, '0')
}

/** Strip everything from `/src-app` on (both separators) so any dir under a
 *  worktree maps to the worktree ROOT — identical to Rust `worktree_root`. */
export function worktreeRoot(dir) {
  for (const marker of ['/src-app', '\\src-app']) {
    const i = dir.indexOf(marker)
    if (i !== -1) return dir.slice(0, i)
  }
  return dir
}

/**
 * The absolute worktree root. Default: `git rev-parse --show-toplevel` from
 * `cwd` (returns the WORKTREE root, not the main checkout — exactly the
 * per-worktree namespace we want). Falls back to `worktreeRoot(cwd)` if git is
 * unavailable, so a non-git tree still keys stably.
 */
export function resolveWorktreeRoot(cwd = process.cwd()) {
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
  return worktreeRoot(cwd)
}

/** The 8-hex run key for a worktree root (default: this cwd's worktree). */
export function worktreeKey(root) {
  const r = root != null ? worktreeRoot(root) : resolveWorktreeRoot()
  return fnv1a8(r)
}

/**
 * Deterministic port BASE (search start) from a key. `floor + (key % span)`.
 * Byte-identical to Rust `port_base`. The key is 8 hex; a malformed key → 0 →
 * base == floor.
 */
export function portBase(key, floor, span) {
  const k = Number.parseInt(key, 16)
  const s = Math.max(1, span >>> 0)
  const off = Number.isFinite(k) ? ((k % s) + s) % s : 0
  return (floor + off) & 0xffff
}

/** True when `port` can be bound on 0.0.0.0 right now (mirrors
 *  `port-manager.ts::isPortBindable`: bind 0.0.0.0 so a docker publish shows). */
export function isPortBindable(port) {
  return new Promise((res) => {
    const srv = createServer()
    srv.once('error', () => res(false))
    srv.once('listening', () => srv.close(() => res(true)))
    srv.listen(port, '0.0.0.0')
  })
}

/**
 * The first OS-bindable port at/after `start` (forward search, `maxAttempts`
 * steps). The key seeds `start`; this makes the FINAL port bind-verified — never
 * a fixed number. Throws if none found (should be impossible with a 200-wide span
 * and few concurrent worktrees).
 */
export async function pickBindablePort(start, maxAttempts = 200) {
  for (let i = 0; i < maxAttempts; i++) {
    const p = ((start + i - 1) % 65535) + 1 // keep in 1..65535
    // eslint-disable-next-line no-await-in-loop
    if (await isPortBindable(p)) return p
  }
  throw new Error(`no bindable port found in ${maxAttempts} attempts from ${start}`)
}

/**
 * Resolve the gallery/dev port with the precedence the audit mandates:
 *   explicit env  >  explicit config `port`  >  key-derived base.
 * A null/undefined `cfgPort` means "derive" (never `NaN`). Returns the BASE
 * (search start) — callers bind-verify via `pickBindablePort` before boot.
 */
export function resolveGalleryPort({
  env,
  cfgPort,
  which = 'webGallery',
  cwd = process.cwd(),
} = {}) {
  const e = env != null ? Number(env) : NaN
  if (Number.isFinite(e) && e > 0) return e | 0
  if (cfgPort != null && Number.isFinite(Number(cfgPort)) && Number(cfgPort) > 0) {
    return Number(cfgPort) | 0
  }
  const { floor, span } = PORT_FLOORS[which] || PORT_FLOORS.webGallery
  return portBase(fnv1a8(resolveWorktreeRoot(cwd)), floor, span)
}

/**
 * No-foreign-reuse predicate (audit §7): a gate:ui/visual/dev run may reuse an
 * already-up server ONLY if it proves same-worktree. `sentinelRoot` is what the
 * running server reports at `/__worktree`; `ownRoot` is this run's worktree root.
 * Anything else (a foreign root, an empty/absent sentinel) → false → boot own.
 */
export function serverIsThisWorktree(sentinelRoot, ownRoot = resolveWorktreeRoot()) {
  if (!sentinelRoot || typeof sentinelRoot !== 'string') return false
  return sentinelRoot.trim() === (ownRoot || '').trim() && sentinelRoot.trim() !== ''
}

/** The sentinel payload the gallery server exposes at `/__worktree`. */
export function sentinelPayload(cwd = process.cwd()) {
  return { worktreeRoot: resolveWorktreeRoot(cwd), pid: process.pid }
}

/** Fetch a running server's sentinel root (or null on any failure/timeout). */
export async function fetchSentinelRoot(port, path = '/__worktree', timeoutMs = 1500) {
  try {
    const ac = new AbortController()
    const t = setTimeout(() => ac.abort(), timeoutMs)
    const r = await fetch(`http://localhost:${port}${path}`, { signal: ac.signal })
    clearTimeout(t)
    if (!r.ok) return null
    const j = await r.json()
    return typeof j?.worktreeRoot === 'string' ? j.worktreeRoot : null
  } catch {
    return null
  }
}
