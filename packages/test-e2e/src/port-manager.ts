/**
 * Cross-session-safe Postgres-port allocator for the E2E global-setup.
 *
 * Multiple concurrent E2E sessions (separate git worktrees) share ONE docker
 * daemon + one loopback port space, so port ownership + container liveness must
 * be judged from a SHARED lock dir keyed by `{pid, runId}` — never a per-worktree
 * file a sibling session can't see. A lock whose owning PID is gone is stale and
 * reapable; a live PID marks the port (and its container) in-use.
 *
 * This is the lean, app-agnostic core the scaffold needs. An app with richer
 * needs (per-worker vite+backend port pairs, heartbeats) keeps its own fixture.
 */
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { tmpdir } from 'node:os'
import { resolve } from 'node:path'

const LOCK_DIR = process.env.ZIEE_E2E_LOCK_DIR || resolve(tmpdir(), 'ziee-test-locks')

interface PgLock {
  pid: number
  runId: string
  port: number
  createdAt: number
}

function ensureLockDir(): void {
  if (!existsSync(LOCK_DIR)) mkdirSync(LOCK_DIR, { recursive: true })
}

/** Is `pid` still alive? (signal 0 only probes existence.) */
function pidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

/** Can we bind `port` on loopback right now? */
function portFree(port: number): Promise<boolean> {
  return new Promise(res => {
    const srv = createServer()
    srv.once('error', () => res(false))
    srv.once('listening', () => srv.close(() => res(true)))
    srv.listen(port, '127.0.0.1')
  })
}

/** Ports currently held by a LIVE lock (any session on this box). */
function liveHeldPorts(): Set<number> {
  ensureLockDir()
  const held = new Set<number>()
  for (const f of readdirSync(LOCK_DIR)) {
    if (!f.startsWith('postgres-') || !f.endsWith('.lock')) continue
    try {
      const lock = JSON.parse(readFileSync(resolve(LOCK_DIR, f), 'utf-8')) as PgLock
      if (pidAlive(lock.pid)) held.add(lock.port)
    } catch {
      /* corrupted lock — ignore */
    }
  }
  return held
}

/** runId → live-pid map, for a container-liveness check in global-setup. */
export function liveRunIds(): Set<string> {
  ensureLockDir()
  const live = new Set<string>()
  for (const f of readdirSync(LOCK_DIR)) {
    if (!f.startsWith('postgres-') || !f.endsWith('.lock')) continue
    try {
      const lock = JSON.parse(readFileSync(resolve(LOCK_DIR, f), 'utf-8')) as PgLock
      if (lock.runId && pidAlive(lock.pid)) live.add(lock.runId)
    } catch {
      /* ignore */
    }
  }
  return live
}

/** Remove lock files whose owning PID is gone. */
export function cleanupStaleLocks(): void {
  ensureLockDir()
  for (const f of readdirSync(LOCK_DIR)) {
    if (!f.endsWith('.lock')) continue
    try {
      const lock = JSON.parse(readFileSync(resolve(LOCK_DIR, f), 'utf-8')) as PgLock
      if (!pidAlive(lock.pid)) rmSync(resolve(LOCK_DIR, f), { force: true })
    } catch {
      rmSync(resolve(LOCK_DIR, f), { force: true })
    }
  }
}

/** Remove per-run config files from crashed runs (whose runId has no live lock). */
export function cleanupStaleConfigFiles(configDir: string): void {
  if (!existsSync(configDir)) return
  const live = liveRunIds()
  for (const f of readdirSync(configDir)) {
    const m = f.match(/^postgres-(.+)\.json$/)
    if (!m) continue
    if (!live.has(m[1])) rmSync(resolve(configDir, f), { force: true })
  }
}

/**
 * Allocate a free Postgres port and write a shared `{pid, runId, port}` lock.
 * Scans upward from `basePort`, skipping ports a live sibling session holds and
 * ports that fail an actual bind probe.
 */
export async function allocatePostgresPort(runId: string, basePort = 54331): Promise<number> {
  cleanupStaleLocks()
  const held = liveHeldPorts()
  for (let port = basePort; port < basePort + 200; port++) {
    if (held.has(port)) continue
    if (!(await portFree(port))) continue
    ensureLockDir()
    const lock: PgLock = { pid: process.pid, runId, port, createdAt: Date.now() }
    writeFileSync(resolve(LOCK_DIR, `postgres-${runId}.lock`), JSON.stringify(lock))
    return port
  }
  throw new Error(`[test-e2e] no free Postgres port in ${basePort}..${basePort + 200}`)
}

/** Release the lock for a given port (called from global-teardown). */
export function releasePostgresPortLock(port: number): void {
  ensureLockDir()
  for (const f of readdirSync(LOCK_DIR)) {
    if (!f.startsWith('postgres-') || !f.endsWith('.lock')) continue
    try {
      const lock = JSON.parse(readFileSync(resolve(LOCK_DIR, f), 'utf-8')) as PgLock
      if (lock.port === port) rmSync(resolve(LOCK_DIR, f), { force: true })
    } catch {
      /* ignore */
    }
  }
}
