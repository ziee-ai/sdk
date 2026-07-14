import * as React from 'react'
import { ChevronRight, Loader2 } from 'lucide-react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { Checkbox } from './checkbox'
import { useControllableState } from './use-controllable-state'
import { cn } from '../lib/utils'
import { type KitStyleProps } from './style-guard'

export interface TreeNode {
  key: string
  title: React.ReactNode
  children?: TreeNode[]
  disabled?: boolean
  /** Marks a node as a leaf so async `loadData` isn't attempted on it. */
  isLeaf?: boolean
}

// Tree: ARIA tree + keyboard nav (roving tabindex; ↑/↓ move, →/← expand/collapse or move,
// Enter/Space select). Controlled OR uncontrolled (default* props). Optional checkboxes with
// parent/child conduction, async lazy-loading (loadData), and opt-in virtualization for big trees.
export type TreeProps = {
  treeData: TreeNode[]
  // expand (controlled `expandedKeys` OR uncontrolled `defaultExpandedKeys`)
  expandedKeys?: string[]
  defaultExpandedKeys?: string[]
  onExpand?: (keys: string[]) => void
  /** Keep a node's ancestors expanded so it stays reachable (default true). */
  autoExpandParent?: boolean
  // select
  selectedKey?: string
  defaultSelectedKey?: string
  onSelect?: (key: string) => void
  // checkboxes (parent/child conduction)
  checkable?: boolean
  checkedKeys?: string[]
  defaultCheckedKeys?: string[]
  onCheck?: (keys: string[]) => void
  /** Lazy-load children on first expand; resolve after the parent has updated `treeData`. */
  loadData?: (node: TreeNode) => Promise<void>
  // virtualization
  virtual?: boolean
  height?: number
  itemHeight?: number
  className?: string
  'aria-label': string
  /** Test selector — forwarded onto <root> (i18n-safe). */
  'data-testid': string
} & KitStyleProps

interface FlatRow { node: TreeNode; level: number; parentKey?: string }

function flattenVisible(nodes: TreeNode[], expanded: Set<string>, level = 1, parentKey?: string, acc: FlatRow[] = []): FlatRow[] {
  for (const n of nodes) {
    acc.push({ node: n, level, parentKey })
    if (n.children?.length && expanded.has(n.key)) flattenVisible(n.children, expanded, level + 1, n.key, acc)
  }
  return acc
}

interface Maps {
  parentOf: Map<string, string | undefined>
  childKeysOf: Map<string, string[]>
  descendantsOf: Map<string, string[]>
}
function buildMaps(treeData: TreeNode[]): Maps {
  const parentOf = new Map<string, string | undefined>()
  const childKeysOf = new Map<string, string[]>()
  const descendantsOf = new Map<string, string[]>()
  const walk = (nodes: TreeNode[], parent?: string): string[] => {
    const acc: string[] = []
    for (const n of nodes) {
      parentOf.set(n.key, parent)
      const kids = n.children ?? []
      childKeysOf.set(n.key, kids.map((k) => k.key))
      const desc = walk(kids, n.key)
      descendantsOf.set(n.key, desc)
      acc.push(n.key, ...desc)
    }
    return acc
  }
  walk(treeData)
  return { parentOf, childKeysOf, descendantsOf }
}

// toggle a node's check + all descendants, then roll up ancestors (all-children-checked ⇒ checked).
function conductCheck(checked: Set<string>, key: string, check: boolean, maps: Maps): Set<string> {
  const next = new Set(checked)
  for (const k of [key, ...(maps.descendantsOf.get(key) ?? [])]) check ? next.add(k) : next.delete(k)
  let p = maps.parentOf.get(key)
  while (p) {
    const kids = maps.childKeysOf.get(p) ?? []
    if (kids.length && kids.every((k) => next.has(k))) next.add(p)
    else next.delete(p)
    p = maps.parentOf.get(p)
  }
  return next
}

export function Tree({
  treeData, expandedKeys, defaultExpandedKeys, onExpand, autoExpandParent = true,
  selectedKey, defaultSelectedKey, onSelect,
  checkable, checkedKeys, defaultCheckedKeys, onCheck, loadData,
  virtual, height = 320, itemHeight = 28, className, style, 'aria-label': ariaLabel,
  'data-testid': testid,
}: TreeProps) {
  const maps = React.useMemo(() => buildMaps(treeData), [treeData])
  // autoExpandParent is a ONE-SHOT seed: expand ancestors of the initial keys, then never widen
  // again (widening on every render would lock a parent open whenever a descendant stays expanded).
  const seededExpanded = React.useMemo(() => {
    const base = defaultExpandedKeys ?? []
    if (!autoExpandParent) return base
    const set = new Set(base)
    for (const k of base) { let p = maps.parentOf.get(k); while (p) { set.add(p); p = maps.parentOf.get(p) } }
    return [...set]
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
  const [expandedArr, setExpanded] = useControllableState<string[]>({
    value: expandedKeys, defaultValue: seededExpanded, onChange: onExpand,
  })
  const [selected, setSelected] = useControllableState<string | undefined>({
    value: selectedKey, defaultValue: defaultSelectedKey, onChange: (k) => onSelect?.(k as string),
  })
  const [checked, setChecked] = useControllableState<string[]>({
    value: checkedKeys, defaultValue: defaultCheckedKeys ?? [], onChange: onCheck,
  })

  const expanded = React.useMemo(() => new Set(expandedArr), [expandedArr])

  const checkedSet = React.useMemo(() => new Set(checked), [checked])
  const halfSet = React.useMemo(() => {
    const half = new Set<string>()
    for (const k of checked) { let p = maps.parentOf.get(k); while (p) { if (!checkedSet.has(p)) half.add(p); p = maps.parentOf.get(p) } }
    return half
  }, [checked, checkedSet, maps])

  const rows = React.useMemo(() => flattenVisible(treeData, expanded), [treeData, expanded])
  const enabled = React.useMemo(() => rows.filter((r) => !r.node.disabled), [rows])
  const [active, setActive] = React.useState<string | undefined>(() => enabled[0]?.node.key)
  const [loadingKeys, setLoadingKeys] = React.useState<Set<string>>(new Set())
  const refs = React.useRef(new Map<string, HTMLDivElement>())
  const scrollRef = React.useRef<HTMLUListElement>(null)

  const virtualizer = useVirtualizer({
    count: rows.length, getScrollElement: () => scrollRef.current, estimateSize: () => itemHeight, overscan: 12,
  })

  React.useEffect(() => {
    if (active == null || !enabled.some((r) => r.node.key === active)) setActive(enabled[0]?.node.key)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rows])

  const pendingFocus = React.useRef<string | null>(null)
  const focus = (key: string) => {
    setActive(key)
    const el = refs.current.get(key)
    if (el) { el.focus(); pendingFocus.current = null; return }
    pendingFocus.current = key
    if (virtual) { const idx = rows.findIndex((r) => r.node.key === key); if (idx >= 0) virtualizer.scrollToIndex(idx) }
  }
  React.useEffect(() => {
    if (pendingFocus.current == null) return
    const el = refs.current.get(pendingFocus.current)
    if (el) { el.focus(); pendingFocus.current = null }
  })

  const toggle = (node: TreeNode) => {
    const key = node.key
    const isOpen = expanded.has(key)
    if (!isOpen && loadData && !node.children?.length && !node.isLeaf && !loadingKeys.has(key)) {
      // lazy load, THEN expand — only on success; clear loading either way. Functional updater so a
      // concurrent expand during the await isn't clobbered.
      setLoadingKeys((s) => new Set(s).add(key))
      void loadData(node)
        .then(() => setExpanded((prev) => (prev.includes(key) ? prev : [...prev, key])))
        .finally(() => setLoadingKeys((s) => { const n = new Set(s); n.delete(key); return n }))
      return
    }
    setExpanded((prev) => (isOpen ? prev.filter((k) => k !== key) : prev.includes(key) ? prev : [...prev, key]))
  }
  const check = (node: TreeNode) => {
    if (node.disabled) return
    const next = conductCheck(checkedSet, node.key, !checkedSet.has(node.key), maps)
    setChecked([...next])
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (active == null) return
    const idx = enabled.findIndex((r) => r.node.key === active)
    const row = enabled[idx]
    if (!row) return
    const hasKids = !!row.node.children?.length || (!!loadData && !row.node.isLeaf)
    const open = expanded.has(row.node.key)
    switch (e.key) {
      case 'ArrowDown': e.preventDefault(); focus(enabled[Math.min(idx + 1, enabled.length - 1)].node.key); break
      case 'ArrowUp': e.preventDefault(); focus(enabled[Math.max(idx - 1, 0)].node.key); break
      case 'ArrowRight':
        e.preventDefault()
        if (hasKids && !open) toggle(row.node)
        else if (hasKids && open) { const next = enabled[idx + 1]; if (next && next.parentKey === row.node.key) focus(next.node.key) }
        break
      case 'ArrowLeft':
        e.preventDefault()
        if (hasKids && open) toggle(row.node)
        else if (row.parentKey) focus(row.parentKey)
        break
      case 'Enter': e.preventDefault(); if (hasKids) toggle(row.node); setSelected(row.node.key); break
      case ' ': e.preventDefault(); if (checkable) check(row.node); else { if (hasKids) toggle(row.node); setSelected(row.node.key) } break
      case 'Home': e.preventDefault(); focus(enabled[0].node.key); break
      case 'End': e.preventDefault(); focus(enabled[enabled.length - 1].node.key); break
    }
  }

  const titleIdFor = (key: string) => `tree-${key}-title`
  const rowEl = (r: FlatRow, virtualStyle?: React.CSSProperties) => {
    const n = r.node
    const hasKids = !!n.children?.length || (!!loadData && !n.isLeaf)
    const open = expanded.has(n.key)
    const isActive = active === n.key
    const loading = loadingKeys.has(n.key)
    const treeitemProps = {
      role: 'treeitem' as const,
      'aria-expanded': hasKids ? open : undefined,
      'aria-selected': selected === n.key || undefined,
      // checked state is exposed by the nested <Checkbox> (avoid double-announcing on the treeitem).
      'aria-level': r.level,
      'aria-disabled': n.disabled || undefined,
    }
    const inner = (
      <div
        ref={(el) => { if (el) refs.current.set(n.key, el); else refs.current.delete(n.key) }}
        tabIndex={n.disabled ? undefined : isActive ? 0 : -1}
        style={{ paddingLeft: `${(r.level - 1) * 1}rem` }}
        className={cn(
          // The whole row is the activation target (it has its own onClick).
          'flex items-center gap-1 rounded px-1 py-0.5 text-sm focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50',
          !n.disabled && 'cursor-pointer hover:bg-accent',
          selected === n.key && 'bg-accent',
          n.disabled && 'opacity-50',
        )}
        onClick={() => { if (n.disabled) return; setActive(n.key); if (hasKids) toggle(n); setSelected(n.key) }}
      >
        {loading
          ? <Loader2 className="size-4 shrink-0 animate-spin opacity-70" aria-hidden />
          : hasKids
            ? <ChevronRight className={cn('size-4 shrink-0 transition-transform', open && 'rotate-90')} aria-hidden />
            : <span className="inline-block size-4 shrink-0" aria-hidden />}
        {checkable && (
          // Row-height, ≥36px flex cell so the checkbox has a comfortable touch
          // target on mobile (the visual box stays 16px; Checkbox's own
          // `::after` expands the precise hit-test area further). The cell also
          // carries the keyboard-focus ring as an INSET box-shadow: an outset
          // ring on the checkbox is clipped by the tree's `overflow-auto` scroll
          // container (G7), whereas an inset ring is drawn inside the box and can
          // never be clipped by an ancestor's overflow.
          <span
            className="flex min-h-9 min-w-9 shrink-0 items-center justify-center rounded-md [&:has(:focus-visible)]:[box-shadow:inset_0_0_0_2px_var(--ring)]"
            onClick={(e) => e.stopPropagation()}
          >
            <Checkbox
              // `ring-0` drops the checkbox's own outset ring — the cell's inset
              // ring is the focus indicator.
              className="size-4 focus-visible:ring-0"
              data-testid={`${testid}-check-${n.key}`}
              checked={checkedSet.has(n.key)}
              indeterminate={halfSet.has(n.key)}
              disabled={n.disabled}
              onCheckedChange={() => check(n)}
              aria-labelledby={titleIdFor(n.key)}
            />
          </span>
        )}
        {/* Wrap (not truncate) so long perm tokens + descriptions stay fully
            legible — a truncated-with-room row is a D1 defect. */}
        <span id={titleIdFor(n.key)} className="min-w-0 flex-1 [overflow-wrap:anywhere]">{n.title}</span>
      </div>
    )
    return virtualStyle
      ? <div key={n.key} {...treeitemProps} style={virtualStyle}>{inner}</div>
      : <li key={n.key} {...treeitemProps}>{inner}</li>
  }

  if (!virtual) {
    return (
      <ul role="tree" aria-label={ariaLabel} aria-multiselectable={checkable || undefined} className={className} style={style} onKeyDown={onKeyDown} data-testid={testid}>
        {rows.map((r) => rowEl(r))}
      </ul>
    )
  }
  const items = virtualizer.getVirtualItems()
  return (
    <ul
      ref={scrollRef}
      role="tree"
      aria-label={ariaLabel}
      aria-multiselectable={checkable || undefined}
      onKeyDown={onKeyDown}
      className={cn('overflow-auto', className)}
      style={{ height, ...style }}
      data-testid={testid}
    >
      <div role="none" style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
        {items.map((vi) => rowEl(rows[vi.index], { position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${vi.start}px)` }))}
      </div>
    </ul>
  )
}
