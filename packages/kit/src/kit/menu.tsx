import * as React from 'react'
import { useSurface } from './surface'
import { type KitStyleProps } from './style-guard'
import { cn } from '../lib/utils'

export type MenuItem =
  | { type: 'divider' }
  | { type: 'group'; label: React.ReactNode; children: MenuItem[] }
  | { type: 'label'; label: React.ReactNode }
  | {
      key: string
      label: React.ReactNode
      icon?: React.ReactNode
      disabled?: boolean
      /** Explicit accessible name — REQUIRED for a non-string label in `collapsed` mode
       *  (the label is hidden then). Preferred over the label/key fallbacks. */
      title?: string
      /** Trailing controls (e.g. a "…" actions dropdown) rendered as a SIBLING of the
       *  item's <button>, never inside it — a <button> may not contain another
       *  interactive control (invalid HTML + a React hydration error). Reveal-on-hover
       *  is the caller's concern via the `group/menu-row` group set on the row <li>.
       *  Ignored in `collapsed` mode (no room on an icon rail). */
      actions?: React.ReactNode
    }

// legacy Menu (navigation subset): vertical/horizontal item list with single selection.
// Rendered as a <nav> + roving list; items are real buttons. `aria-label` required (i18n).
export type MenuProps = {
  items: MenuItem[]
  /** Selected item key (single-selection). */
  selectedKey?: string
  /** Selected item keys (alias of `selectedKey`; any match marks the item current). */
  selectedKeys?: string[]
  /** Keys of items that are an ANCESTOR of the current page (a broader section
   *  you're within, but not the exact current page). Rendered with a subtle
   *  "active section" treatment instead of the strong selected pill, and WITHOUT
   *  `aria-current="page"` — so a section + its open sub-item don't both claim to
   *  be the current page. An item present in both selected and ancestor is
   *  treated as selected (exact wins). */
  ancestorKeys?: string[]
  onSelect?: (key: string) => void
  mode?: 'vertical' | 'horizontal'
  /** Icon-only rail: hides labels (the label becomes each item's accessible name). */
  collapsed?: boolean
  className?: string
  'aria-label': string
  /** Test selector — forwarded onto <root>. Items derive `${testid}-item-${key}`. */
  'data-testid': string
} & KitStyleProps

// Extract a plain-text accessible name from a label node (used when `collapsed` hides it).
function labelText(label: React.ReactNode): string | undefined {
  return typeof label === 'string' ? label : undefined
}

/**
 * Row style tokens for a single Menu row, so a VIRTUALIZED list that must render
 * pixel-faithful menu rows (the sidebar Recent-chats list) shares ONE source of
 * truth with the kit `<Menu>` instead of re-deriving the classes (which would
 * silently drift). Returns the three class strings the row is composed of:
 *  - `row`     — the highlight/hover container (was the `<li>` className). Sets the
 *                `relative` positioning context + `group/menu-row` hover group when
 *                `hasActions`, and the selected/ancestor/hover colours.
 *  - `button`  — the transparent full-width row button.
 *  - `actions` — the trailing actions overlay (absolute, right-aligned, masked).
 * The kit `<Menu>` puts `row` on the `<li>`; the virtualized list puts it on an
 * inner `<div>` (the `<li>` itself is absolutely positioned by the virtualizer).
 */
export function menuRowClasses(opts: {
  selected?: boolean
  ancestor?: boolean
  hasActions?: boolean
  collapsed?: boolean
}): { row: string; button: string; actions: string } {
  const { selected, ancestor, hasActions, collapsed } = opts
  return {
    row: cn(
      'rounded-md',
      // relative anchors the absolutely-overlaid actions (see `actions`).
      hasActions && 'group/menu-row relative',
      // Row-LEVEL highlight (not button-level) so it spans the whole row incl.
      // the trailing actions. Hover is OPAQUE (not /60) so the actions'
      // `bg-inherit` mask paints the same colour without double-darkening.
      selected
        ? 'bg-primary text-primary-foreground font-medium'
        : ancestor
          ? 'bg-accent text-accent-foreground font-medium'
          : 'hover:bg-accent',
    ),
    button: cn(
      // Transparent: the visible highlight lives on the row container above.
      'flex w-full min-w-0 items-center gap-2 rounded-md text-sm bg-transparent',
      collapsed ? 'justify-center px-2 py-1.5' : 'px-3 py-1.5',
      // Inset focus ring: menu rows live in scrollable rails flush to the viewport
      // edge, where an OUTSET ring gets clipped by the scroll container.
      'focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-inset focus-visible:ring-ring/50 disabled:opacity-50',
    ),
    // ps-1: the mask is ~the kebab's own width — it must NOT eat a wide strip of label.
    actions:
      'absolute inset-y-0 end-0 flex items-center pe-1 ps-1 rounded-e-md bg-inherit pointer-events-none',
  }
}

/**
 * The interactive row button of a menu row, as a kit component so consumers
 * outside `components/ui` (e.g. the virtualized sidebar Recent-chats list) get
 * the EXACT same button element + `menuRowClasses().button` styling without a
 * raw `<button>` (which the noRestrictedInteractive guardrail forbids in module
 * code, and which would drift from this styling). Selected marks `aria-current`
 * (the row-level colour highlight lives on the wrapping `menuRowClasses().row`
 * element, applied by the caller).
 */
export function MenuRowButton({
  selected,
  disabled,
  title,
  onClick,
  className,
  children,
  'data-testid': testid,
  'aria-label': ariaLabel,
}: {
  selected?: boolean
  disabled?: boolean
  title?: string
  onClick?: () => void
  className?: string
  children: React.ReactNode
  'data-testid'?: string
  'aria-label'?: string
}) {
  return (
    <button
      type="button"
      disabled={disabled}
      data-testid={testid}
      aria-current={selected ? 'page' : undefined}
      aria-label={ariaLabel}
      title={title}
      onClick={onClick}
      className={cn(menuRowClasses({}).button, className)}
    >
      {children}
    </button>
  )
}

function Items({ items, selectedSet, ancestorSet, onSelect, locked, collapsed, itemTestid, groupTestid }: {
  items: MenuItem[]
  selectedSet: Set<string>
  ancestorSet: Set<string>
  onSelect?: (k: string) => void
  locked: boolean
  collapsed: boolean
  itemTestid?: (key: string) => string | undefined
  groupTestid?: (index: number) => string | undefined
}) {
  return (
    <>
      {items.map((it, i) => {
        // The <li> must keep its implicit listitem role (a role="separator" on the
        // li makes the parent <ul> contain a non-listitem child → axe `list`
        // violation). Put the separator on an inner element instead.
        if ('type' in it && it.type === 'divider')
          return (
            <li key={`d${i}`} className="my-1">
              <div role="separator" className="h-px bg-border" />
            </li>
          )
        if ('type' in it && it.type === 'group') {
          return (
            <li key={`g${i}`} data-testid={groupTestid?.(i)}>
              {/* group caption is decorative chrome — hidden in the collapsed rail. */}
              {!collapsed && <div className="px-3 py-1 text-xs font-medium text-muted-foreground">{it.label}</div>}
              <ul className="contents">
                <Items items={it.children} selectedSet={selectedSet} ancestorSet={ancestorSet} onSelect={onSelect} locked={locked} collapsed={collapsed} itemTestid={itemTestid} groupTestid={groupTestid} />
              </ul>
            </li>
          )
        }
        // Non-interactive caption row (legacy antd `{ type: 'group' }` standalone label).
        if ('type' in it && it.type === 'label') {
          return collapsed ? null : (
            <li key={`l${i}`} className="px-3 py-1 text-xs font-medium text-muted-foreground">
              {it.label}
            </li>
          )
        }
        const item = it
        const selected = selectedSet.has(item.key)
        // Ancestor = you're within this section but it isn't the exact page.
        // Exact selection always wins over an ancestor match.
        const ancestor = !selected && ancestorSet.has(item.key)
        // Never nameless in collapsed mode: explicit title → string label → the key.
        const name = item.title ?? labelText(item.label) ?? item.key
        // Trailing actions render as a SIBLING of the button (a <button> can't nest an
        // interactive control). `group/menu-row` on the <li> lets those actions reveal
        // on row hover/focus. Suppressed on the collapsed icon rail.
        const hasActions = item.actions != null && !collapsed
        // Shared row style tokens (also used by the virtualized sidebar list).
        const rowCls = menuRowClasses({ selected, ancestor, hasActions, collapsed })
        return (
          <li key={item.key} className={rowCls.row}>
            <button
              type="button"
              disabled={item.disabled || locked}
              data-testid={itemTestid?.(item.key)}
              aria-current={selected ? 'page' : undefined}
              // collapsed hides the text → the label becomes the button's accessible name + tooltip.
              aria-label={collapsed ? name : undefined}
              title={collapsed ? name : undefined}
              onClick={() => onSelect?.(item.key)}
              className={rowCls.button}
            >
              {item.icon != null && <span aria-hidden className="shrink-0 [&_svg]:size-4">{item.icon}</span>}
              {/* truncate long labels instead of overflowing the rail. */}
              {!collapsed && <span className="min-w-0 flex-1 truncate text-left">{item.label}</span>}
            </button>
            {hasActions && (
              // Overlay the trailing actions on the row's right edge. bg-inherit
              // paints the row's own highlight over the label's tail; pointer-events-none
              // lets clicks on the masked strip fall through to the row button below.
              <div className={rowCls.actions}>{item.actions}</div>
            )}
          </li>
        )
      })}
    </>
  )
}

export function Menu({ items, selectedKey, selectedKeys, ancestorKeys, onSelect, mode = 'vertical', collapsed = false, className, style, 'aria-label': ariaLabel, 'data-testid': testid }: MenuProps) {
  const s = useSurface({})
  const itemTestid = React.useCallback(
    (k: string) => (testid ? `${testid}-item-${k}` : undefined),
    [testid],
  )
  const groupTestid = React.useCallback(
    (index: number) => (testid ? `${testid}-group-${index}` : undefined),
    [testid],
  )
  // Merge the single + multi selection inputs into one O(1) lookup set.
  const selectedSet = React.useMemo(() => {
    const set = new Set(selectedKeys ?? [])
    if (selectedKey != null) set.add(selectedKey)
    return set
  }, [selectedKey, selectedKeys])
  const ancestorSet = React.useMemo(() => new Set(ancestorKeys ?? []), [ancestorKeys])
  return (
    <nav aria-label={ariaLabel} style={style} data-testid={testid}>
      <ul className={cn(mode === 'horizontal' ? 'flex items-center gap-1' : 'flex flex-col gap-0.5', className)}>
        <Items items={items} selectedSet={selectedSet} ancestorSet={ancestorSet} onSelect={onSelect} locked={!!s.disabled} collapsed={collapsed} itemTestid={itemTestid} groupTestid={groupTestid} />
      </ul>
    </nav>
  )
}
