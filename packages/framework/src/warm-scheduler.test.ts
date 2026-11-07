import { test } from 'node:test'
import assert from 'node:assert/strict'
import { getWarmScheduler, setWarmScheduler, type WarmTaskMeta } from './warm-scheduler.ts'
import { __autoWarmLazyActionsForTests } from './store-kit.ts'

// The warm-scheduler seam (fix/preload-storm): with an app scheduler
// installed, store-kit hands it ONE task per lazy action instead of firing
// every action's chunk import in one idle callback; the task runs the
// action's `preload()` only when the scheduler calls it.

test('WS-1: store-kit hands each lazy action to the installed scheduler, keyed <store>.<action>', async () => {
  const tasks: Array<{ run: () => unknown; meta: WarmTaskMeta }> = []
  setWarmScheduler((run, meta) => tasks.push({ run, meta }))
  try {
    const preloaded: string[] = []
    const dispatchers = {
      load: { preload: async () => void preloaded.push('load') },
      save: { preload: async () => void preloaded.push('save') },
    }
    __autoWarmLazyActionsForTests('Library', dispatchers)
    assert.deepEqual(
      tasks.map(t => t.meta),
      [
        { source: 'store-actions', key: 'Library.load' },
        { source: 'store-actions', key: 'Library.save' },
      ],
    )
    // Nothing is fetched until the scheduler decides to run a task.
    assert.deepEqual(preloaded, [])
    await tasks[1].run()
    assert.deepEqual(preloaded, ['save'])
  } finally {
    setWarmScheduler(null)
  }
})

test('WS-2: with no scheduler installed the getter answers null (the legacy idle path runs)', () => {
  setWarmScheduler(null)
  assert.equal(getWarmScheduler(), null)
})
