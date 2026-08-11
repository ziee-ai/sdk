import * as React from 'react'
import { Plus, X } from 'lucide-react'
import { Tabs as Root, TabsList, TabsTrigger, TabsContent } from '../shadcn/tabs'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { DivScrollX } from '../internal/div-scroll-x'
import { cn } from '../lib/utils'

export interface TabItem {
  key: string
  label: React.ReactNode
  children?: React.ReactNode
  disabled?: boolean
  /** Show a close (×) affordance on this tab. Defaults to true when the Tabs is `editable`. */
  closable?: boolean
}

interface TabsBase {
  items: TabItem[]
  value?: string
  defaultValue?: string
  onValueChange?: (value: string) => void
  /** Fires when a tab trigger is clicked (legacy `onTabClick`), even if already active. */
  onTabClick?: (key: string) => void
  disabled?: boolean
  size?: 'sm' | 'default'
  /** Tab strip look. `default` = boxed/segmented pill strip; `line` = quiet
   *  UNDERLINE tabs (no box). Per the design system, dense/narrow containers
   *  (side panels, toolbars) should use `line` — a boxed strip reads as heavy in
   *  a narrow side panel (taxonomy J5). */
  variant?: 'default' | 'line'
  className?: string
  /** Fill the container: root becomes a flex column, the tab strip stays a fixed
   *  row, and the active panel gets the remaining height AS A FLEX COLUMN — so a
   *  panel's content can lay itself out against that height and scroll inside it.
   *  Use for a tabbed viewer that must fit a bounded box. */
  fill?: boolean
  /** Extra classes on the tab strip row (e.g. `justify-center px-3`). */
  tabStripClassName?: string
  /** Scroll the tab strip with the app's OverlayScrollbars (auto-hide) instead
   *  of a native `overflow-x-auto` box. */
  scrollX?: boolean
  /** Test selector — forwarded onto <root>. Triggers derive `${testid}-tab-${key}`, panels `${testid}-panel-${key}`. */
  'data-testid': string
}
// Editable-card mode renders an add button + per-tab close affordances. The add button
// has no other handler, so `onEdit` is REQUIRED here — a type error otherwise prevents
// shipping editable tabs whose add/close buttons silently do nothing.
interface TabsEditable {
  /** Editable-card mode (legacy `type="editable-card"`). Requires `onEdit`. */
  editable: true
  /** Unified edit handler (legacy antd `onEdit`): action is 'add' (key='') or 'remove'. */
  onEdit: (action: 'add' | 'remove', key: string) => void
  /** Hide the add button while keeping per-tab close affordances. */
  hideAdd?: boolean
  /** Also fires with the key of the tab whose close affordance was activated. */
  onClose?: (key: string) => void
  /** Accessible name for the add button. Falls back to "Add tab" if omitted. */
  addLabel?: string
  /** Accessible name for a tab's close affordance. Falls back to "Close <label>". */
  closeLabel?: (item: TabItem) => string
}
interface TabsStatic {
  editable?: false
  onEdit?: never
  hideAdd?: never
  onClose?: never
  addLabel?: never
  closeLabel?: never
}
export type TabsProps = TabsBase & (TabsEditable | TabsStatic)

export function Tabs({
  items, value, defaultValue, onValueChange, onTabClick, disabled, size, variant, className, fill,
  tabStripClassName, scrollX,
  editable, hideAdd, onEdit, onClose, addLabel, closeLabel, 'data-testid': testid,
}: TabsProps) {
  // React to an ambient disabled surface (e.g. inside a disabled Form/Card).
  const s = useSurface({ disabled })

  // Track the active key (controlled or uncontrolled) so we can emit a stable,
  // primitive-independent `data-state="active"|"inactive"` hook on each trigger:
  // Base UI marks the active tab with a bare `data-active`, but tests assert the
  // old Radix `data-state` vocabulary. Mirrors kit/segmented.tsx.
  const [current, setCurrent] = useControllableState<string>({
    value,
    defaultValue: defaultValue ?? items[0]?.key ?? '',
    onChange: onValueChange,
  })

  const remove = (item: TabItem) => {
    onClose?.(item.key)
    onEdit?.('remove', item.key)
  }
  const add = () => onEdit?.('add', '')

  return (
    <Root
      value={current}
      onValueChange={(v) => setCurrent(String(v ?? ''))}
      className={cn('w-full', fill && 'flex flex-col min-h-0', className)}
      data-testid={testid}
    >
      {/* The add button lives OUTSIDE TabsList: role=tablist requires its
          children to be role=tab (aria-required-children), and a plain add button
          inside it violates that. The strip scrolls horizontally (native
          overflow-x-auto, or the app's OverlayScrollbars via `scrollX`) so a long
          tab strip scrolls instead of wrapping/clipping. */}
      {scrollX ? (
        <DivScrollX className={cn(fill && 'shrink-0')} contentClassName={tabStripClassName}>
          {renderStrip()}
        </DivScrollX>
      ) : (
        <div className={cn('flex items-center overflow-x-auto', fill && 'shrink-0', tabStripClassName)}>
          {renderStrip()}
        </div>
      )}
      {items.map((t) => (
        <TabsContent
          key={t.key}
          value={t.key}
          data-testid={testid ? `${testid}-panel-${t.key}` : undefined}
          // A flex COLUMN, not a bounded block. `flex-1 min-h-0 overflow-hidden` alone gives the
          // panel the right height and the right clipping and leaves it `display: block` — under
          // which every `flex-1` / `min-h-0` a child states is INERT. A panel hosting a pane with
          // its own internal structure (a scrolling body plus something pinned beside it — the
          // shape most panes have) then laid that pane out at its full content height and the
          // panel's own `overflow-hidden` swallowed the excess: content CLIPPED, with no track
          // anywhere to scroll it back. Measured at 390px: an 896px pane in a 688px panel, 208px
          // of it — including its primary action — unreachable rather than merely below the fold.
          //
          // Safe to make `flex` because base-ui's Tabs.Panel is `keepMounted={false}`: an
          // inactive panel is ABSENT, never present-and-`hidden` (where an author-level
          // `display: flex` would out-cascade the UA's `[hidden] { display: none }` and draw
          // every panel at once). Pinned, with the box contract, in `tabs-fill-panel.test.tsx`.
          className={fill ? 'flex flex-1 min-h-0 flex-col overflow-hidden' : undefined}
        >
          {t.children}
        </TabsContent>
      ))}
    </Root>
  )

  function renderStrip() {
    return (
      <>
      <TabsList variant={variant}>
        {items.map((t) => {
          const showClose = (t.closable ?? editable) && !s.disabled && !t.disabled
          // The close affordance is a REAL sibling <button>, never nested inside the
          // TabsTrigger <button> (button-in-button is invalid + keyboard-unreachable).
          // A native button gives Enter/Space + Tab focus for free.
          return (
            <div key={t.key} className="relative inline-flex items-center">
              <TabsTrigger
                value={t.key}
                disabled={t.disabled || s.disabled}
                data-testid={testid ? `${testid}-tab-${t.key}` : undefined}
                data-state={current === t.key ? 'active' : 'inactive'}
                onClick={() => onTabClick?.(t.key)}
                className={cn(size === 'sm' && 'px-2 py-1 text-xs', showClose && 'pr-7')}
              >
                {t.label}
              </TabsTrigger>
              {showClose && (
                <button
                  type="button"
                  aria-label={closeLabel ? closeLabel(t) : `Close ${typeof t.label === 'string' ? t.label : t.key}`}
                  // stop the activation from also selecting the tab.
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={(e) => { e.stopPropagation(); remove(t) }}
                  className="absolute right-1.5 top-1/2 inline-flex size-4 -translate-y-1/2 items-center justify-center rounded-sm opacity-60 hover:bg-accent hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
                >
                  <X className="size-3" aria-hidden />
                </button>
              )}
            </div>
          )
        })}
      </TabsList>
      {editable && !hideAdd && (
        <button
          type="button"
          aria-label={addLabel ?? 'Add tab'}
          disabled={s.disabled}
          onClick={add}
          className="ml-1 inline-flex size-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 disabled:opacity-50"
        >
          <Plus className="size-4" aria-hidden />
        </button>
      )}
      </>
    )
  }
}
