/**
 * HOST LOCK — serialize gallery crawls across every worktree on one machine.
 *
 * ## Why this exists
 *
 * Two agents running `gate:ui` concurrently on one host silently corrupted each
 * other, with no warning in either output: one run produced 95.5% contaminated
 * findings while a serialized run of the same commit produced zero. The
 * mechanism is now proven (see `runtime-health.mjs`'s validity gate): a
 * concurrent run can take the gallery origin away mid-crawl — by winning a port
 * race, by a `pkill -f vite` (which the repo's own troubleshooting docs
 * recommend), or simply by killing its own server in a `finally` that the other
 * run's browser is still pulling modules from. Every request in flight then
 * fails, the console mirror of each failure is not classified as harness noise,
 * and the run reports thousands of transport errors as product defects.
 *
 * **Per-worktree isolation does not help.** Separate `node_modules`, separate
 * build databases and separate key-derived ports all leave the two runs sharing
 * one machine's network namespace and process table, which is where they
 * collide. So the lock is scoped to the HOST, deliberately.
 *
 * ## Design
 *
 * `O_EXCL` create of a single well-known file, holding a JSON holder record.
 * Node has no portable `flock(2)`, and an `O_EXCL` file does not vanish when its
 * owner is SIGKILLed — so the record carries the holder's `pid` and an acquirer
 * that finds the lock held probes `process.kill(pid, 0)` and reclaims when the
 * holder is gone. That mirrors the "prove it is OURS before reusing" discipline
 * `run-key.mjs` already applies to a dev-server port.
 *
 * Scoped per-USER (`ziee-gate-ui-<uid>.lock`) because `os.tmpdir()` is shared:
 * a lock file owned by another user could not be unlinked, which would wedge the
 * host for everyone — a worse failure than the one being fixed.
 */
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

/** A holder older than this is treated as abandoned even if its pid is alive
 *  (a wedged process must not be able to block the host forever). Comfortably
 *  longer than any observed gate run (~12 min for 682 cells). */
export const MAX_HOLD_MS = 2 * 60 * 60 * 1000

/** Env var by which a PARENT passes ownership to a child it spawns. Without it,
 *  `gate-ui` would deadlock against its own `runtime-health` child. */
export const TOKEN_ENV = 'GATE_UI_LOCK_TOKEN'
/** Set to "0" to disable the lock entirely (used by this feature's own
 *  concurrency NEGATIVE control, which must be able to produce an overlap). */
export const DISABLE_ENV = 'GATE_UI_LOCK'

export function lockPath() {
  // `os.userInfo().username` works on every platform. The previous fallback was
  // the literal string 'win' when `process.getuid` was absent, which gave every
  // Windows/CI user on a shared TEMP one shared, mutually-unlinkable lock file —
  // reintroducing the cross-user wedge the per-user scoping exists to avoid.
  let who
  try {
    who = typeof process.getuid === 'function' ? process.getuid() : os.userInfo().username
  } catch {
    who = 'unknown'
  }
  return path.join(os.tmpdir(), `ziee-gate-ui-${String(who).replace(/[^\w.-]/g, '_')}.lock`)
}

const alive = pid => {
  if (!Number.isInteger(pid) || pid <= 0) return false
  try {
    process.kill(pid, 0)
    return true
  } catch (e) {
    // EPERM means the process exists but belongs to someone else — still alive.
    return e.code === 'EPERM'
  }
}

/**
 * True if ANY process this lock is responsible for is still running.
 *
 * This is deliberately not just `holder.pid`. `gate-ui` holds the lock but runs
 * the actual crawl as a CHILD process; if gate-ui is SIGKILLed the child is
 * reparented and keeps driving Chromium against the dev server for the rest of
 * the crawl. Checking only the holder would then declare the lock stale and let
 * a SECOND crawl start alongside the orphan — exactly the concurrent-crawl
 * corruption the lock exists to prevent, blessed as a "stale reclaim". So a
 * child registers its own pid via `registerWorker`, and the lock stays held
 * while EITHER is alive.
 */
const anyAlive = holder =>
  alive(holder?.pid) || (holder?.workers ?? []).some(alive)

function readHolder(file) {
  try {
    const raw = fs.readFileSync(file, 'utf-8')
    if (!raw.trim()) return undefined // present but EMPTY — see tryCreate
    return JSON.parse(raw)
  } catch (e) {
    if (e.code === 'ENOENT') return null
    return undefined // corrupt/partial — distinguished from "absent" on purpose
  }
}

/**
 * Why a held lock may be taken over. Returns null when it may NOT.
 *
 * An UNREADABLE record (`undefined`) is treated as *possibly mid-write*, not as
 * garbage: the writer creates the file and fills it in two steps, so a reader
 * landing in that window sees an empty file. Judging that "corrupt" and
 * unlinking it would delete a lock a peer had just legitimately taken — two
 * simultaneous holders. The caller therefore re-reads before acting on it.
 */
function staleReason(holder) {
  if (holder === undefined) return null // mid-write or transiently unreadable
  if (!holder || typeof holder !== 'object') return 'holder record is unreadable'
  if (!anyAlive(holder)) {
    const w = (holder.workers ?? []).length
    return `holder pid ${holder.pid}${w ? ` (and ${w} worker pid(s))` : ''} is no longer running`
  }
  const age = Date.now() - (Number(holder.startedAt) || 0)
  if (age > MAX_HOLD_MS)
    return `holder pid ${holder.pid} has held the lock ${Math.round(age / 60000)}min (> ${MAX_HOLD_MS / 60000}min)`
  return null
}

/**
 * Create the lock ATOMICALLY, content and all: write a private temp file, then
 * `link()` it into place. `link` fails with EEXIST if the target exists, so the
 * winner's file is never observable in a half-written state — closing the
 * window in which a peer could read an empty record, judge it corrupt, and
 * unlink it.
 */
function tryCreate(file, record) {
  const tmp = `${file}.${process.pid}.${Date.now().toString(36)}.tmp`
  try {
    fs.writeFileSync(tmp, JSON.stringify(record), { flag: 'wx' })
    fs.linkSync(tmp, file)
    return true
  } catch (e) {
    if (e.code === 'EEXIST') return false
    throw e
  } finally {
    try {
      fs.unlinkSync(tmp)
    } catch {
      /* already gone */
    }
  }
}

/**
 * Reclaim a stale lock SAFELY: only unlink if the file still contains the exact
 * record we judged stale. Two waiters wake within the same second after a holder
 * dies, so a bare read-then-unlink lets the second one delete the first's
 * freshly-created lock and both proceed.
 */
function reclaimIfUnchanged(file, judgedToken) {
  const now = readHolder(file)
  if (!now || now.token !== judgedToken) return false // someone else already won
  try {
    fs.unlinkSync(file)
    return true
  } catch {
    return false
  }
}

/**
 * Record an additional pid whose liveness keeps this lock held — used by a
 * parent to register the CHILD that does the real work, so SIGKILLing the parent
 * cannot make the lock look free while the child is still crawling.
 */
export function registerWorker(token, workerPid, file = lockPath()) {
  const holder = readHolder(file)
  if (!holder || holder.token !== token) return false
  holder.workers = [...new Set([...(holder.workers ?? []), workerPid])]
  const tmp = `${file}.${process.pid}.reg.tmp`
  try {
    fs.writeFileSync(tmp, JSON.stringify(holder))
    fs.renameSync(tmp, file) // atomic replace
    return true
  } catch {
    try {
      fs.unlinkSync(tmp)
    } catch {
      /* ignore */
    }
    return false
  }
}

/**
 * Acquire the host lock.
 *
 * @returns {Promise<{held:boolean, inherited?:boolean, disabled?:boolean,
 *                    token?:string, release:()=>void, waitedMs:number}>}
 *
 * Never returns silently-unlocked: the caller always learns which of
 * held / inherited / disabled applies, so "two runs overlapped and nobody said
 * anything" — the observed failure — is not representable.
 */
export async function acquire(opts = {}) {
  const {
    owner = process.cwd(),
    wait = true,
    waitMs = 15 * 60 * 1000,
    pollMs = 1000,
    log = m => console.log(m),
    env = process.env,
  } = opts

  if (env[DISABLE_ENV] === '0') {
    log(`• host lock DISABLED via ${DISABLE_ENV}=0 — concurrent runs may interfere`)
    return { held: false, disabled: true, waitedMs: 0, release: () => {} }
  }
  if (env[TOKEN_ENV]) {
    // A child of a holder. Do NOT contend: the parent already serialized us.
    // The token is VALIDATED against the on-disk lock, because it is a plain env
    // var: once it leaks into a shell (an `export` while debugging, an agent
    // harness that snapshots a child env) an unvalidated token would silently
    // disable the lock for every later run — "two runs overlapped and nobody
    // said anything", which is the defect, not the fix.
    const holder = readHolder(lockPath())
    if (holder && holder.token === env[TOKEN_ENV]) {
      log(`• inheriting the host lock from pid ${holder.pid} (${holder.owner})`)
      // Register OUR pid so the lock stays held if the parent is SIGKILLed while
      // we keep crawling.
      registerWorker(env[TOKEN_ENV], process.pid)
      return {
        held: false,
        inherited: true,
        token: env[TOKEN_ENV],
        waitedMs: 0,
        release: () => {},
      }
    }
    log(
      `• ${TOKEN_ENV} is set but does not match any live lock holder — ignoring it ` +
        `and contending normally (a stale token in the environment must not silently ` +
        `disable serialization).`,
    )
    // fall through and contend
  }

  const file = lockPath()
  const token = `${process.pid}-${Date.now().toString(36)}`
  const record = { pid: process.pid, owner, token, startedAt: Date.now() }
  const started = Date.now()
  let announced = false

  for (;;) {
    if (tryCreate(file, record)) {
      if (announced) log(`• host lock acquired after ${Math.round((Date.now() - started) / 1000)}s`)
      return {
        held: true,
        token,
        waitedMs: Date.now() - started,
        release: () => releaseIfOurs(file, token),
      }
    }

    const holder = readHolder(file)
    if (holder === null) continue // vanished between our create attempt and now
    if (holder === undefined) {
      // Present but unreadable — most likely mid-write by the winner. Give it a
      // moment and re-read rather than declaring it corrupt and unlinking it.
      await new Promise(r => setTimeout(r, 50))
      const again = readHolder(file)
      if (again === undefined) {
        log('• host lock: record is unreadable on two reads — reclaiming')
        try {
          fs.unlinkSync(file)
        } catch {
          /* someone else got there first */
        }
      }
      continue
    }
    const stale = staleReason(holder)
    if (stale) {
      // Loud, never silent: a reclaim means someone's run died mid-crawl.
      // Verify-then-unlink, so two waiters waking in the same second cannot both
      // reclaim (the second would otherwise delete the first's new lock).
      log(`• host lock: reclaiming stale lock — ${stale}`)
      reclaimIfUnchanged(file, holder.token)
      continue
    }

    if (!wait) {
      const err = new Error(
        `gate:ui is already running on this host (pid ${holder.pid}, ${holder.owner}). ` +
          `Concurrent runs corrupt each other's findings. Wait for it, or re-run ` +
          `without --no-wait, or set ${DISABLE_ENV}=0 to opt out deliberately.`,
      )
      err.holder = holder
      throw err
    }
    if (Date.now() - started > waitMs) {
      const err = new Error(
        `timed out after ${Math.round(waitMs / 1000)}s waiting for the host lock held by ` +
          `pid ${holder.pid} (${holder.owner}).`,
      )
      err.holder = holder
      throw err
    }
    if (!announced) {
      announced = true
      log(
        `• waiting for the gallery host lock — held by pid ${holder.pid} ` +
          `(${holder.owner}). Concurrent crawls corrupt each other, so this run ` +
          `will start when that one finishes.`,
      )
    }
    await new Promise(r => setTimeout(r, pollMs))
  }
}

/** Release only if WE still hold it — never clobber a lock reclaimed from us. */
export function releaseIfOurs(file, token) {
  const holder = readHolder(file)
  if (holder && holder.token !== token) return false
  try {
    fs.unlinkSync(file)
    return true
  } catch {
    return false
  }
}

/**
 * Acquire, run `fn`, release on return and on throw.
 *
 * ## Why there are NO SIGTERM/SIGINT handlers here
 *
 * The obvious design — catch the signals and unlink the lock — is actively
 * harmful in this program. Registering a listener SUPPRESSES node's default
 * terminate action, and `gate-ui` blocks its event loop inside `spawnSync` for
 * the entire ~12-minute crawl, so the listener cannot run until the crawl
 * finishes anyway. The net effect measured on a minimal repro: SIGTERM was
 * ignored for the full duration of the blocking child, i.e. `gate:ui` became
 * UNKILLABLE for twelve minutes where it previously died instantly — and the
 * operator who then escalates to SIGKILL skips the handler regardless. The
 * handlers cost the ability to stop a wedged run and buy nothing in the only
 * window that matters.
 *
 * So a signalled run simply dies and leaves its lock file behind. That is SAFE
 * by construction: the next acquirer's liveness check finds the holder (and its
 * registered workers) gone and reclaims it, loudly. The stale-reclaim path is
 * the mechanism — not a fallback for one, and it is exercised by TEST-16c.
 * `process.on('exit')` is kept because it costs nothing and covers the ordinary
 * `process.exit()` paths (`finish()` in gate-ui) that skip `finally`.
 */
export async function withHostLock(opts, fn) {
  const lock = await acquire(opts)
  let released = false
  const release = () => {
    if (released) return
    released = true
    lock.release()
  }
  process.on('exit', release)
  try {
    return await fn(lock)
  } finally {
    release()
    process.off('exit', release)
  }
}
