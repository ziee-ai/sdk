/**
 * Unit tests for the settings-field guardrail — in particular RULE 2
 * (`starved-label-column`), added after `/settings/scheduler` shipped with every
 * label wrapped to one word per line while RULE 1 was satisfied and green.
 *
 *   node --test sdk/packages/config/src/lint/settings-field.test.mjs
 */
import test from 'node:test'
import assert from 'node:assert/strict'

import { scanSource, isScoped } from './settings-field.mjs'

const SETTINGS_FILE = '/repo/src/modules/scheduler/pages/SchedulerAdminPage.tsx'
const PLAIN_FILE = '/repo/src/modules/scheduler/components/TaskCard.tsx'

const rules = src => scanSource(SETTINGS_FILE, src).map(v => v.rule)

test('isScoped recognises the three settings scopes and nothing else', () => {
  assert.equal(isScoped('/repo/src/modules/scheduler/pages/SchedulerAdminPage.tsx'), true)
  assert.equal(isScoped('/repo/src/modules/settings-general/GeneralSettings.tsx'), true)
  assert.equal(isScoped('/repo/src/modules/file-rag/components/sections/Limits.tsx'), true)
  assert.equal(isScoped('/repo/src/modules/file/components/FileCard.tsx'), false)
})

test('RULE 2 fires on the exact markup that shipped broken', () => {
  // verbatim shape of the pre-fix SchedulerAdminPage row
  const src = `
    export function P() {
      return (
        <Field orientation="horizontal">
          <FieldContent><FieldTitle>Max active tasks per user</FieldTitle></FieldContent>
          <InputNumber data-testid="scheduler-max-active" min={1} max={1000} />
        </Field>
      )
    }`
  assert.deepEqual(rules(src), ['starved-label-column'])
})

test('RULE 2 fires for every stretching control, in both beside-orientations', () => {
  for (const tag of ['Input', 'InputNumber', 'InputPassword', 'Textarea']) {
    for (const o of ['horizontal', 'responsive']) {
      const src = `export const P = () => (
        <Field orientation="${o}"><FieldContent><FieldTitle>A label here</FieldTitle></FieldContent><${tag} /></Field>)`
      assert.deepEqual(rules(src), ['starved-label-column'], `${tag} @ ${o}`)
    }
  }
})

test('RULE 2 does NOT fire inside a FormField (the canonical composition)', () => {
  const src = `
    export const P = () => (
      <Form form={f} layout="horizontal" onSubmit={s} data-testid="x">
        <FormField name="a" label="Max active tasks per user" description="…">
          <InputNumber data-testid="scheduler-max-active" className="w-40" />
        </FormField>
      </Form>)`
  assert.deepEqual(rules(src), [])
})

test('RULE 2 does NOT fire on intrinsic-width controls in a beside Field (the legal shadcn row)', () => {
  // ScheduledTaskFormDrawer (Switch), ThemeSettings / McpToolApprovalsTab (Select,
  // Segmented) — all measured at 0 starved labels.
  for (const tag of ['Switch', 'Checkbox', 'Segmented', 'RadioGroup', 'Select']) {
    const src = `export const P = () => (
      <Field orientation="responsive"><FieldContent><FieldTitle>Show a toast when it runs</FieldTitle></FieldContent><${tag} /></Field>)`
    assert.deepEqual(rules(src), [], tag)
  }
})

test('RULE 2 does NOT fire on a vertical Field (label above the control)', () => {
  const src = `export const P = () => (
    <Field><FieldTitle>Run at</FieldTitle><Input /></Field>)`
  assert.deepEqual(rules(src), [])
})

test('RULE 2 does NOT fire outside a settings-scoped file', () => {
  const src = `export const P = () => (
    <Field orientation="horizontal"><FieldContent><FieldTitle>A label here</FieldTitle></FieldContent><Input /></Field>)`
  assert.deepEqual(scanSource(PLAIN_FILE, src).map(v => v.rule), ['starved-label-column'])
  // scanSource itself is scope-agnostic; the SCOPE filter lives in scanRoots/isScoped.
  assert.equal(isScoped(PLAIN_FILE), false)
})

test('the data-standalone-control opt-out silences both rules', () => {
  const beside = `export const P = () => (
    <Field orientation="horizontal"><FieldContent><FieldTitle>A label here</FieldTitle></FieldContent><Input data-standalone-control /></Field>)`
  assert.deepEqual(rules(beside), [])
  const bare = `export const P = () => (<div className="flex flex-col gap-2"><Input data-standalone-control /></div>)`
  assert.deepEqual(rules(bare), [])
})

test('RULE 1 still fires for a control outside any Field', () => {
  const src = `export const P = () => (
    <div className="flex flex-col gap-2"><label>Name</label><Input /></div>)`
  assert.deepEqual(rules(src), ['control-outside-field'])
})

test('a nested FormField inside a beside Field is still exempt', () => {
  const src = `export const P = () => (
    <Field orientation="horizontal">
      <FieldContent><FieldTitle>Group label</FieldTitle></FieldContent>
      <FormField name="n" label="Inner"><Input /></FormField>
    </Field>)`
  assert.deepEqual(rules(src), [])
})
