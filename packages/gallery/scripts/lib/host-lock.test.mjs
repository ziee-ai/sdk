/**
 * TEST — the host lock (TEST-16/17/18).
 *
 * Run: node --test scripts/lib/host-lock.test.mjs
 */
import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawn } from 'node:child_process'
import { fileURLToPath } from 'node:url'
import {
  MAX_HOLD_MS,
  TOKEN_ENV,
  DISABLE_ENV,
  acquire,
  lockPath,
  registerWorker,
  releaseIfOurs,
  withHostLock,
} from './host-lock.mjs'

const HERE = path.dirname(fileURLToPath(import.meta.url))
const quiet = () => {}

/** Run the real lock against an isolated TMPDIR so tests never touch the host's
 *  actual lock (and never serialize against a real gate:ui run). */
function isolated(fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'hostlock-'))
  const prev = { TMPDIR: process.env.TMPDIR, TMP: process.env.TMP, TEMP: process.env.TEMP }
  process.env.TMPDIR = process.env.TMP = process.env.TEMP = dir
  return Promise.resolve()
    .then(() => fn(dir))
    .finally(() => {
      for (const [k, v] of Object.entries(prev))
        if (v === undefined) delete process.env[k]
        else process.env[k] = v
      fs.rmSync(dir, { recursive: true, force: true })
    })
}

test('TEST-16 acquire on a free lock succeeds and records pid + owner', () =>
  isolated(async () => {
    const lock = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    assert.equal(lock.held, true)
    const rec = JSON.parse(fs.readFileSync(lockPath(), 'utf-8'))
    assert.equal(rec.pid, process.pid)
    assert.equal(rec.owner, '/wt/a')
    assert.ok(rec.token && rec.startedAt)
    lock.release()
    assert.equal(fs.existsSync(lockPath()), false, 'release removes the lock file')
  }))

test('TEST-16b a second acquire with wait:false REFUSES, naming the holder', () =>
  isolated(async () => {
    const first = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    await assert.rejects(
      () => acquire({ owner: '/wt/b', wait: false, log: quiet, env: {} }),
      e =>
        /already running on this host/.test(e.message) &&
        e.message.includes('/wt/a') &&
        e.holder.pid === process.pid,
      'the refusal must identify the holding worktree + pid — silence is the defect',
    )
    first.release()
  }))

test('TEST-16c a lock whose holder pid is DEAD is reclaimed, not waited on', () =>
  isolated(async () => {
    // pid 2^22 is above any Linux default pid_max and is not running.
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({ pid: 4194303, owner: '/wt/ghost', token: 't', startedAt: Date.now() }),
    )
    const msgs = []
    const lock = await acquire({ owner: '/wt/b', log: m => msgs.push(m), env: {} })
    assert.equal(lock.held, true, 'a dead holder must not block')
    assert.ok(
      msgs.some(m => /reclaiming stale lock/.test(m) && /no longer running/.test(m)),
      'the reclaim must be logged loudly, never silent',
    )
    lock.release()
  }))

test('TEST-16d a CORRUPT lock record is reclaimed rather than wedging the host', () =>
  isolated(async () => {
    fs.writeFileSync(lockPath(), 'not json at all {{{')
    const msgs = []
    const lock = await acquire({ owner: '/wt/b', log: m => msgs.push(m), env: {} })
    assert.equal(lock.held, true)
    assert.ok(msgs.some(m => /unreadable/.test(m)))
    lock.release()
  }))

test('TEST-16e a live but ANCIENT holder is reclaimed (a wedged run cannot block forever)', () =>
  isolated(async () => {
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({
        pid: process.pid, // alive
        owner: '/wt/wedged',
        token: 't',
        startedAt: Date.now() - MAX_HOLD_MS - 1000,
      }),
    )
    const msgs = []
    const lock = await acquire({ owner: '/wt/b', log: m => msgs.push(m), env: {} })
    assert.equal(lock.held, true)
    assert.ok(msgs.some(m => /has held the lock/.test(m)))
    lock.release()
  }))

test('TEST-16f release does NOT clobber a lock that was reclaimed from us', () =>
  isolated(async () => {
    const lock = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    // Someone else reclaimed and re-took it:
    fs.writeFileSync(
      lockPath(),
      JSON.stringify({ pid: process.pid, owner: '/wt/other', token: 'OTHER', startedAt: Date.now() }),
    )
    lock.release()
    assert.equal(fs.existsSync(lockPath()), true, "we must not delete another run's lock")
    assert.equal(releaseIfOurs(lockPath(), 'OTHER'), true)
  }))

test('TEST-17 a child carrying the parent TOKEN inherits and does NOT contend', () =>
  isolated(async () => {
    const parent = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    // The child sees the token in its env — it must return immediately, not block.
    const child = await acquire({
      owner: '/wt/a',
      wait: false,
      log: quiet,
      env: { [TOKEN_ENV]: parent.token },
    })
    assert.equal(child.inherited, true)
    assert.equal(child.held, false)
    assert.equal(child.token, parent.token)
    // …and its release must be a no-op, or it would free the PARENT's lock.
    child.release()
    assert.equal(fs.existsSync(lockPath()), true, "the child must not free the parent's lock")
    parent.release()
  }))

test('TEST-17b a child WITHOUT the token does contend (the token is load-bearing)', () =>
  isolated(async () => {
    const parent = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    await assert.rejects(
      () => acquire({ owner: '/wt/a', wait: false, log: quiet, env: {} }),
      /already running on this host/,
    )
    parent.release()
  }))

test(`TEST-16g ${DISABLE_ENV}=0 opts out and says so`, () =>
  isolated(async () => {
    const msgs = []
    const lock = await acquire({ log: m => msgs.push(m), env: { [DISABLE_ENV]: '0' } })
    assert.equal(lock.disabled, true)
    assert.equal(fs.existsSync(lockPath()), false)
    assert.ok(msgs.some(m => /DISABLED/.test(m) && /may interfere/.test(m)))
  }))

test('TEST-18 withHostLock releases on normal return AND on throw', () =>
  isolated(async () => {
    await withHostLock({ log: quiet, env: {} }, async () => 'ok')
    assert.equal(fs.existsSync(lockPath()), false, 'released on normal return')

    await assert.rejects(() =>
      withHostLock({ log: quiet, env: {} }, async () => {
        throw new Error('boom')
      }),
    )
    assert.equal(fs.existsSync(lockPath()), false, 'released on throw')
  }))

test('TEST-18b a SIGKILLed/SIGTERMed holder does not wedge the host — the next acquirer RECLAIMS', () =>
  isolated(async dir => {
    // CONTRACT CHANGE, deliberate. An earlier version registered SIGTERM/SIGINT
    // handlers to unlink the lock. That was actively harmful: registering a
    // listener SUPPRESSES node's default terminate action, and `gate-ui` blocks
    // its event loop in `spawnSync` for the whole ~12-minute crawl, so the
    // handler could not run until the crawl finished anyway. Net effect: gate:ui
    // became UNKILLABLE by SIGTERM for twelve minutes where it previously died
    // instantly — and an operator escalating to SIGKILL skipped the handler
    // regardless.
    //
    // So a signalled run now simply dies and LEAVES its lock file. What must
    // hold — and what this test proves — is that the next acquirer is not
    // blocked by it: the liveness check finds the holder gone and reclaims,
    // loudly. That is the mechanism, not a fallback for one.
    const script = path.join(dir, 'holder.mjs')
    fs.writeFileSync(
      script,
      `import { withHostLock } from ${JSON.stringify(path.join(HERE, 'host-lock.mjs'))}
withHostLock({ owner: '/wt/child', log: () => {}, env: {} }, async () => {
  const keepalive = setInterval(() => {}, 1000)
  console.log('HELD')
  await new Promise(() => {})
  clearInterval(keepalive)
})\n`,
    )
    const child = spawn(process.execPath, [script], {
      env: { ...process.env, TMPDIR: dir, TMP: dir, TEMP: dir },
      stdio: ['ignore', 'pipe', 'inherit'],
    })
    await new Promise((res, rej) => {
      const t = setTimeout(() => rej(new Error('child never signalled HELD')), 15000)
      child.stdout.on('data', d => {
        if (String(d).includes('HELD')) {
          clearTimeout(t)
          res()
        }
      })
    })
    assert.equal(fs.existsSync(lockPath()), true, 'child holds the lock')

    // SIGKILL is the harsher, more honest case: no handler can possibly run.
    child.kill('SIGKILL')
    await new Promise(res => child.on('exit', res))
    await new Promise(r => setTimeout(r, 200))

    const msgs = []
    const next = await acquire({ owner: '/wt/next', wait: false, log: m => msgs.push(m), env: {} })
    assert.equal(next.held, true, 'a killed holder must NOT block the next run')
    assert.ok(
      msgs.some(m => /reclaiming stale lock/.test(m) && /no longer running/.test(m)),
      'and the reclaim must be announced, not silent',
    )
    next.release()
  }))

test('TEST-18c a lock whose WORKER child is still alive is NOT reclaimed', () =>
  isolated(async dir => {
    // The hole a blind reviewer found: gate-ui holds the lock but the crawl runs
    // in a spawned CHILD. SIGKILL the parent and the child is reparented and
    // keeps driving Chromium against the dev server — yet a holder-pid-only
    // liveness check would call the lock stale and let a SECOND crawl start
    // beside the orphan, which is exactly the corruption the lock prevents.
    // A worker registers itself, and the lock stays held while EITHER is alive.
    const worker = spawn(process.execPath, ['-e', 'setInterval(()=>{},1000)'], {
      stdio: 'ignore',
    })
    try {
      const holder = await acquire({ owner: '/wt/a', log: quiet, env: {} })
      assert.equal(registerWorker(holder.token, worker.pid), true)
      // Simulate the PARENT dying: rewrite the record with a dead holder pid but
      // keep the live worker.
      const rec = JSON.parse(fs.readFileSync(lockPath(), 'utf-8'))
      rec.pid = 4194303 // not running
      fs.writeFileSync(lockPath(), JSON.stringify(rec))

      await assert.rejects(
        () => acquire({ owner: '/wt/b', wait: false, log: quiet, env: {} }),
        /already running on this host/,
        'the orphaned crawl still holds the machine — a second crawl must not start',
      )

      // Once the worker dies too, the lock becomes reclaimable.
      worker.kill('SIGKILL')
      await new Promise(res => worker.on('exit', res))
      const next = await acquire({ owner: '/wt/b', wait: false, log: quiet, env: {} })
      assert.equal(next.held, true)
      next.release()
    } finally {
      try {
        worker.kill('SIGKILL')
      } catch {
        /* already gone */
      }
    }
  }))

test('TEST-16h contention actually SERIALIZES: the waiter starts only after release', () =>
  isolated(async () => {
    const first = await acquire({ owner: '/wt/a', log: quiet, env: {} })
    let acquiredAt = 0
    const waiter = acquire({ owner: '/wt/b', pollMs: 25, log: quiet, env: {} }).then(l => {
      acquiredAt = Date.now()
      return l
    })
    await new Promise(r => setTimeout(r, 250))
    assert.equal(acquiredAt, 0, 'the waiter must NOT have acquired while held')
    const releasedAt = Date.now()
    first.release()
    const second = await waiter
    assert.equal(second.held, true)
    assert.ok(
      acquiredAt >= releasedAt,
      'the second run must begin only after the first released',
    )
    second.release()
  }))
