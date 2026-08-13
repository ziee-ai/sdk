import * as React from 'react'
import { Accordion as Root, AccordionItem, AccordionTrigger, AccordionContent } from '../shadcn/accordion'
import { useSurface } from './surface'
import { cn } from '../lib/utils'

export interface AccordionItemDef {
  key: string
  label: React.ReactNode
  children?: React.ReactNode
  disabled?: boolean
  /** Header-right action rendered OUTSIDE the toggle, so clicking it does not
   *  expand/collapse the panel (e.g. an always-visible "Assign"/"Edit" button). */
  extra?: React.ReactNode
}

/** Keep a CLOSED panel's children MOUNTED (hidden), instead of unmounting them.
 *
 *  Base UI defaults this to `false`, so by default a collapsed panel's subtree is
 *  destroyed: its effects run their cleanups, its component state is lost, and anything
 *  it had published to a store is retracted. That is the right default for a disclosure
 *  whose body is inert content, and the WRONG one wherever collapse is meant to be
 *  DISPLAY rather than reset — a panel holding live state, a published badge, or an
 *  ordering the host reads from the mounted child. Opt in there; the default is unchanged
 *  for every existing consumer. */
type KeepMounted = { keepMounted?: boolean }

export type AccordionProps =
  | ({ items: AccordionItemDef[]; type?: 'single'; collapsible?: boolean; defaultValue?: string; value?: string; onValueChange?: (v: string) => void; ghost?: boolean; className?: string; 'data-testid': string } & KeepMounted)
  | ({ items: AccordionItemDef[]; type: 'multiple'; defaultValue?: string[]; value?: string[]; onValueChange?: (v: string[]) => void; ghost?: boolean; className?: string; 'data-testid': string } & KeepMounted)

// legacy `ghost` removes item borders/background.
const ghostCls = '[&_[data-slot=accordion-item]]:border-0'

function renderItems(items: AccordionItemDef[], surfaceDisabled: boolean) {
  return items.map((it) => (
    <AccordionItem key={it.key} value={it.key} disabled={it.disabled || surfaceDisabled}>
      <AccordionTrigger extra={it.extra}>{it.label}</AccordionTrigger>
      <AccordionContent>{it.children}</AccordionContent>
    </AccordionItem>
  ))
}

export function Accordion(props: AccordionProps) {
  const { items, className, ghost } = props
  const testid = props['data-testid']
  const cls = cn(ghost && ghostCls, className)
  // react to an ambient disabled surface (e.g. inside a disabled Form/Card).
  const s = useSurface({})
  // Base UI Accordion is value-array based with `openMultiple` (no `type` /
  // `collapsible`). Multiple = array passthrough; single = wrap the string in a
  // 1-tuple and unwrap on change.
  if (props.type === 'multiple') {
    return (
      <Root multiple value={props.value} defaultValue={props.defaultValue}
        keepMounted={props.keepMounted}
        onValueChange={props.onValueChange ? (v) => props.onValueChange!(v as string[]) : undefined}
        className={cls} data-testid={testid}>
        {renderItems(items, !!s.disabled)}
      </Root>
    )
  }
  return (
    <Root multiple={false}
      keepMounted={props.keepMounted}
      value={props.value != null ? [props.value] : undefined}
      defaultValue={props.defaultValue != null ? [props.defaultValue] : undefined}
      onValueChange={props.onValueChange ? (v) => props.onValueChange!((v as string[])[0] ?? '') : undefined}
      className={cls} data-testid={testid}>
      {renderItems(items, !!s.disabled)}
    </Root>
  )
}
