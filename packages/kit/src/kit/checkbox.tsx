import * as React from 'react'
import { Checkbox as Base } from '../shadcn/checkbox'
import { Skeleton } from '../shadcn/skeleton'
import { Tooltip } from './tooltip'
import { useSurface } from './surface'
import { cn } from '../lib/utils'
import type { CheckedBinding } from './value-binding'
import type { NoUndeclaredAria } from './aria-passthrough'

interface CheckboxBase {
  /** Mixed state (legacy `indeterminate`); overrides `checked` visually until toggled. */
  indeterminate?: boolean
  onBlur?: () => void
  disabled?: boolean
  name?: string
  id?: string
  label?: React.ReactNode
  className?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-describedby'?: string
  /** Marks the control required for assistive tech. `FormField required` INJECTS this (via
   *  cloneElement, untyped), so a control that neither declares nor forwards it drops it
   *  silently — the same silent-drop class `aria-passthrough.ts` exists for, and one the type
   *  ban cannot see because the injection is untyped. */
  'aria-required'?: boolean
  /**
   * WHY this checkbox is unavailable. Presence means unavailable — the string IS the reason,
   * rather than a boolean plus a separate message that can disagree with it. The same contract
   * as `Button.unavailableReason`, for the same reasons; see that prop's doc for the full
   * rationale, and USE IT INSTEAD OF `disabled` WHENEVER THERE IS A REASON TO GIVE.
   *
   * A checkbox needs this MORE than a button does, not less. The places a tick box gets refused
   * are overwhelmingly inside a popover or a menu — a column chooser, a facet list, a
   * permission tree — where there is no room for adjacent explanatory text and no other control
   * to hang it off. A greyed tick box in a popover is a refusal with literally nowhere for a
   * reason to live, unless the control carries it.
   *
   * Renders `aria-disabled` with the toggle swallowed, so it stays hoverable and focusable, and
   * wires the reason into BOTH channels: the TOOLTIP for a pointer user, and an sr-only node
   * referenced by `aria-describedby` for everyone else. The NAME (`label` / `aria-label`) is
   * untouched — the reason says why it refused, the name still says WHICH box refused.
   */
  unavailableReason?: string
  invalid?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
}
// Controlled `checked` requires a change handler (see CheckedBinding); FormField stays valid.
export type CheckboxProps = CheckboxBase & NoUndeclaredAria<CheckboxBase> & CheckedBinding

export const Checkbox = React.forwardRef<HTMLButtonElement, CheckboxProps>(function Checkbox(
  { checked, value, defaultChecked, indeterminate, onCheckedChange, onChange, onBlur, disabled, name, id, label, className,
    'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby, 'aria-describedby': ariaDescribedby, 'aria-required': ariaRequired,
    unavailableReason, invalid,
    'data-testid': testid },
  ref,
) {
  const s = useSurface({ disabled })
  const reactId = React.useId()
  const ctrlId = id ?? reactId
  // Presence means unavailable, and an EMPTY string is not a reason — a caller that computed
  // its reason to '' has said there is nothing to state, and must not silently get an inert
  // control out of it.
  const unavailable = unavailableReason != null && unavailableReason !== ''
  // `useId` rather than a hand-built key: the reason node's `id` must be UNIQUE IN THE DOCUMENT
  // or the browser resolves `aria-describedby` to whichever node came first, and one row's
  // reason announces another row's. A chooser renders one of these PER COLUMN, so that is the
  // normal case here, not an edge one.
  const whyId = React.useId()
  const handle = (v: boolean | 'indeterminate') => {
    // `aria-disabled` is a claim to assistive tech, not an enforcement — without this the
    // control would still toggle, which is the trap that makes people reach for native
    // `disabled` and lose the tooltip and the tab stop with it.
    if (unavailable) return
    const b = v === true
    onCheckedChange?.(b)
    onChange?.(b)
  }
  if (s.loading) return <Skeleton className={cn('size-4 rounded', className)} />
  const control = (
    <Base
      ref={ref}
      id={ctrlId}
      checked={checked ?? value}
      indeterminate={indeterminate}
      defaultChecked={defaultChecked}
      onCheckedChange={handle}
      onBlur={onBlur}
      // `unavailableReason` OVERRIDES a native disable: a caller that supplied a reason has said
      // the control must be able to state it, and a natively-disabled control can state nothing.
      disabled={(s.disabled || s.readOnly) && !unavailable}
      aria-disabled={unavailable || undefined}
      name={name}
      aria-label={label == null ? ariaLabel : undefined}
      aria-labelledby={ariaLabelledby}
      // The caller's own description is KEPT, never silently replaced by the kit's.
      aria-describedby={
        unavailable ? [ariaDescribedby, whyId].filter(Boolean).join(' ') : ariaDescribedby
      }
      aria-invalid={invalid || undefined}
      aria-required={ariaRequired || undefined}
      data-testid={testid}
      className={cn(unavailable && 'cursor-not-allowed opacity-50', className)}
    />
  )
  // The reason in BOTH channels. The sr-only node is a SIBLING (a checkbox renders no children
  // of its own to hide one in, unlike Button) — `sr-only` is absolutely positioned, so it is out
  // of flow and adds no flex item to the row below and no box to the bare form.
  //
  // THE TRIGGER IS A WRAPPING SPAN, NOT THE CHECKBOX ITSELF — and unlike Switch (which uses the
  // wrapper to avoid a flicker on a tiny hit area) here it is the only thing that WORKS.
  // `Tooltip.Trigger` keys its store on the id it was GIVEN, and base-ui then merges the rendered
  // element's own props over its computed ones. `Checkbox.Root` GENERATES ITS OWN `id`
  // (`base-ui-_r_N_`) and overrides both the caller's and the tooltip's, so a tooltip hung
  // directly on the control is registered under an id nothing on screen carries: it stamps
  // `data-base-ui-tooltip-trigger`, reads as wired in source AND in markers, and opens nothing.
  // Measured, not assumed — the DOM shows `id="base-ui-_r_4_"` where `ctrlId` was passed.
  // (`w-fit` for the same reason Switch documents: inside a `flex flex-col` field the wrapper
  // would otherwise stretch and anchor the tooltip mid-row instead of over the 16px box.)
  const reachable = unavailable ? (
    <>
      <Tooltip content={unavailableReason}>
        <span className="inline-flex w-fit">{control}</span>
      </Tooltip>
      <span id={whyId} className="sr-only">
        {unavailableReason}
      </span>
    </>
  ) : (
    control
  )
  if (label == null) return reachable
  // sibling label (NOT nested) — nesting + htmlFor double-fires the toggle.
  return (
    <div className="flex items-center gap-2">
      {reachable}
      <label htmlFor={ctrlId} className={cn('text-sm', (s.disabled || unavailable) && 'opacity-60')}>{label}</label>
    </div>
  )
})
