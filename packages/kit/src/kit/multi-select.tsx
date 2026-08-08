import * as React from 'react'
import { Check, ChevronsUpDown, Plus } from 'lucide-react'
import { useVirtualizer } from '@tanstack/react-virtual'
import { Popover as Root, PopoverTrigger, PopoverContent } from '../shadcn/popover'
import { optionListPopupWidth } from '../shadcn/popup-width'
import { Command, CommandInput, CommandList, CommandEmpty, CommandGroup, CommandItem } from '../shadcn/command'
import { Skeleton } from '../shadcn/skeleton'
import { Tag } from './tag'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { type KitStyleProps } from './style-guard'
import type { ValueBinding } from './value-binding'
import { cn } from '../lib/utils'

export interface MultiSelectOption {
  label: string
  value: string
  disabled?: boolean
  /** Optional heading this option sits under. Options carrying the same `group` are rendered
   *  together beneath it, in first-appearance order; options with none keep today's flat list.
   *  Additive: a caller that passes no `group` gets byte-identical behaviour.
   *
   *  A heading rather than a label suffix, because a suffix goes on the END of the option text —
   *  which is exactly where tail-truncation removes it, so the part that distinguishes two
   *  similar options is the first thing to disappear. */
  group?: string
  /** What the SELECTED TAG shows, when that should differ from the row's `label` — the mirror
   *  of `SelectOption.selectedLabel` on the single Select.
   *
   *  It exists because the row and the tag are read in different places: a row sits UNDER its
   *  `group` heading, which already says where it came from, while a tag stands alone in the
   *  trigger with no heading above it. Without this the tag can only repeat the bare row label,
   *  so two options distinguished ONLY by their heading collapse into two identical tags — the
   *  choice is legible while it is being made and illegible the moment it is made.
   *
   *  A node rather than a string, so the caller can decide what truncates and what must not.
   *  Additive: an option that omits it renders exactly the `label` tag it always did. The
   *  remove button's accessible name still comes from `label` (a string is what it needs). */
  selectedLabel?: React.ReactNode
}

// Split a raw input string on the configured token separators, returning the cleaned tokens
// found (everything except the trailing in-progress fragment) plus that remainder.
function splitOnSeparators(text: string, separators: string[]): { tokens: string[]; rest: string } {
  if (separators.length === 0) return { tokens: [], rest: text }
  const escaped = separators.map((s) => s.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('')
  const parts = text.split(new RegExp(`[${escaped}]`))
  const rest = parts.pop() ?? ''
  const tokens = parts.map((p) => p.trim()).filter(Boolean)
  return { tokens, rest }
}


/** Split options into heading-bearing runs, in first-appearance order.
 *
 *  One anonymous run when nothing declares a `group`, so the ungrouped case renders exactly the
 *  single `CommandGroup` it always did. */
function groupOptions(options: MultiSelectOption[]): { heading?: string; options: MultiSelectOption[] }[] {
  if (!options.some((o) => o.group)) return [{ options }]
  const out: { heading?: string; options: MultiSelectOption[] }[] = []
  const byHeading = new Map<string, { heading?: string; options: MultiSelectOption[] }>()
  for (const o of options) {
    const key = o.group ?? ''
    let run = byHeading.get(key)
    if (!run) {
      run = { heading: o.group, options: [] }
      byHeading.set(key, run)
      out.push(run)
    }
    run.options.push(o)
  }
  return out
}

// Virtualized multi-select listbox for large option sets (cmdk renders all rows + can't window).
// Own filter + arrow-key aria-activedescendant nav; toggling keeps the popover open.
function VirtualMultiList({
  options, selectedSet, onToggle, searchPlaceholder, emptyText, listboxId,
  allowCreate, tokenSeparators, onCreateToken, createLabel,
  optionTestid,
}: {
  options: MultiSelectOption[]
  selectedSet: Set<string>
  onToggle: (value: string) => void
  searchPlaceholder: string
  emptyText: string
  listboxId: string
  allowCreate?: boolean
  tokenSeparators: string[]
  onCreateToken: (value: string) => void
  createLabel: (query: string) => string
  optionTestid?: (value: string) => string | undefined
}) {
  const [query, setQuery] = React.useState('')
  const filtered = React.useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? options.filter((o) => o.label.toLowerCase().includes(q)) : options
  }, [options, query])
  const firstEnabled = React.useMemo(() => filtered.findIndex((o) => !o.disabled), [filtered])
  const lastEnabled = React.useMemo(() => { for (let i = filtered.length - 1; i >= 0; i--) if (!filtered[i].disabled) return i; return -1 }, [filtered])
  const [active, setActive] = React.useState(0)
  React.useEffect(() => { setActive(firstEnabled >= 0 ? firstEnabled : 0) }, [firstEnabled])
  const scrollRef = React.useRef<HTMLDivElement>(null)
  const inputRef = React.useRef<HTMLInputElement>(null)
  React.useEffect(() => { inputRef.current?.focus() }, [])
  const virtualizer = useVirtualizer({
    count: filtered.length, getScrollElement: () => scrollRef.current, estimateSize: () => 32, overscan: 8,
  })
  const trimmed = query.trim()
  const exists = React.useMemo(
    () => options.some((o) => o.value === trimmed || o.label === trimmed) || selectedSet.has(trimmed),
    [options, selectedSet, trimmed],
  )
  const canCreate = !!allowCreate && trimmed !== '' && !exists
  const commit = (v: string) => { onCreateToken(v); setQuery('') }
  const nextEnabled = (from: number, dir: 1 | -1) => {
    for (let i = from + dir; i >= 0 && i < filtered.length; i += dir) if (!filtered[i].disabled) return i
    return -1
  }
  const goTo = (n: number) => { if (n >= 0) { setActive(n); virtualizer.scrollToIndex(n) } }
  const onChange = (raw: string) => {
    if (allowCreate && tokenSeparators.length) {
      const { tokens, rest } = splitOnSeparators(raw, tokenSeparators)
      tokens.forEach(commit)
      setQuery(rest)
    } else {
      setQuery(raw)
    }
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); goTo(nextEnabled(active, 1)) }
    else if (e.key === 'ArrowUp') { e.preventDefault(); goTo(nextEnabled(active, -1)) }
    else if (e.key === 'Home') { e.preventDefault(); goTo(firstEnabled) }
    else if (e.key === 'End') { e.preventDefault(); goTo(lastEnabled) }
    else if (e.key === 'Enter') {
      e.preventDefault()
      const o = filtered[active]
      if (o && !o.disabled) { onToggle(o.value); return }
      // typed text that matches an option's value OR label selects that option (not a new token).
      const exact = options.find((op) => op.value === trimmed || op.label === trimmed)
      if (exact && !exact.disabled) { onToggle(exact.value); return }
      if (canCreate) commit(trimmed)
    }
  }
  const virtualItems = virtualizer.getVirtualItems()
  const activeMounted = virtualItems.some((vi) => vi.index === active)
  return (
    <div>
      <div className="flex items-center border-b px-3">
        <input
          ref={inputRef}
          role="combobox"
          aria-expanded
          aria-controls={listboxId}
          aria-autocomplete="list"
          aria-activedescendant={activeMounted && filtered[active] ? `${listboxId}-${active}` : undefined}
          value={query}
          onChange={(e) => onChange(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder={searchPlaceholder}
          className="h-10 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground"
        />
      </div>
      {/* fixed create row above the virtualized scroller (it can't host a non-option row). */}
      {canCreate && (
        <button
          type="button"
          onClick={() => commit(trimmed)}
          className="flex w-full items-center gap-2 px-3 py-2 text-sm hover:bg-accent focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 outline-none"
        >
          <Plus className="size-4 shrink-0" aria-hidden />
          <span className="truncate">{createLabel(trimmed)}</span>
        </button>
      )}
      {filtered.length === 0 && !canCreate ? (
        <div className="py-6 text-center text-sm text-muted-foreground">{emptyText}</div>
      ) : filtered.length === 0 ? null : (
        <div ref={scrollRef} role="listbox" aria-multiselectable id={listboxId} className="max-h-72 overflow-auto p-1">
          <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
            {virtualItems.map((vi) => {
              const o = filtered[vi.index]
              const selected = selectedSet.has(o.value)
              const isActive = active === vi.index
              return (
                <div
                  key={o.value}
                  id={`${listboxId}-${vi.index}`}
                  data-testid={optionTestid?.(o.value)}
                  role="option"
                  aria-selected={selected}
                  aria-disabled={o.disabled || undefined}
                  onMouseMove={() => setActive(vi.index)}
                  onClick={() => { if (!o.disabled) onToggle(o.value) }}
                  style={{ position: 'absolute', top: 0, left: 0, width: '100%', transform: `translateY(${vi.start}px)` }}
                  className={cn(
                    'flex h-8 cursor-pointer items-center gap-2 rounded-sm px-2 text-sm',
                    isActive && 'bg-accent',
                    o.disabled && 'pointer-events-none opacity-50',
                  )}
                >
                  <Check className={cn('size-4 shrink-0', selected ? 'opacity-100' : 'opacity-0')} aria-hidden />
                  {/* NOTE the VIRTUALIZED rows are absolutely positioned, so they contribute
                      nothing to the popup's intrinsic width — a `virtual` list therefore rests
                      on the trigger floor rather than growing to its content. That is the same
                      width it had before `optionListPopupWidth` existed, so nothing regresses;
                      widening it would mean measuring rows the virtualizer has deliberately not
                      mounted. */}
                  <span className="truncate">{o.label}</span>
                </div>
              )
            })}
          </div>
        </div>
      )}
    </div>
  )
}

/**
 * Keep a TAG'S OWN REMOVE CONTROL from reaching the combobox trigger it sits inside — and
 * nothing else.
 *
 * The remove button is the one part of a tag whose activation must not also open the list:
 * pressing `×` means "drop this value", and a popover opening on top of that is noise at
 * best. Every other part of the tag is, to a user, part of the control — clicking it is
 * clicking the combobox — so it must bubble.
 *
 * The test is on the EVENT TARGET rather than on the handler's placement, because the two
 * differ: a handler on the wrapper sees the events of everything inside it. `data-slot` is
 * the kit's own structural marker (the tag span already carries one), so this asks the DOM
 * which part was hit instead of assuming the wrapper only wraps the button.
 */
const stopFromTagClose = (e: React.SyntheticEvent): void => {
  if (e.target instanceof Element && e.target.closest('[data-slot="tag-close"]') != null) {
    e.stopPropagation()
  }
}

// Multi-select with searchable list + removable tags (legacy Select mode="multiple").
// `allowCreate` enables free-text tokens not present in `options` (legacy Select mode="tags").
// Form-bindable: value:string[] + onChange(string[]) + name + id + ref.
interface MultiSelectBase {
  options: MultiSelectOption[]
  onBlur?: () => void
  placeholder: string
  searchPlaceholder: string
  emptyText: string
  /** Builds the accessible name for a tag's remove button, e.g. (label) => `Remove ${label}`. */
  removeLabel: (label: string) => string
  /** Allow adding free-text values not in `options` (legacy Select mode="tags"). */
  allowCreate?: boolean
  /** Characters that commit the typed text as a token (e.g. [',']). Used with `allowCreate`. */
  tokenSeparators?: string[]
  /** Label for the "create" affordance. Falls back to `Create "<query>"`. */
  createLabel?: (query: string) => string
  disabled?: boolean
  loading?: boolean
  invalid?: boolean
  /** Window the option list for large sets (own filter + keyboard nav, replaces cmdk). */
  virtual?: boolean
  name?: string
  id?: string
  className?: string
  'aria-describedby'?: string
  'aria-required'?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). Options derive `${testid}-opt-${value}`. */
  'data-testid': string
}
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type MultiSelectProps = MultiSelectBase &
  ValueBinding<string[]> &
  KitStyleProps &
  // An accessible name is REQUIRED — either an inline label or a referenced one (no silent default).
  (
    | { 'aria-label': string; 'aria-labelledby'?: never }
    | { 'aria-labelledby': string; 'aria-label'?: never }
  )

export const MultiSelect = React.forwardRef<HTMLDivElement, MultiSelectProps>(function MultiSelect(
  { options, value, defaultValue, onValueChange, onChange, onBlur, placeholder, searchPlaceholder, emptyText, removeLabel,
    allowCreate, tokenSeparators = [], createLabel, disabled, loading, invalid, virtual, name, id, className, style,
    'aria-describedby': ariaDescribedby, 'aria-label': ariaLabel, 'aria-labelledby': ariaLabelledby,
    'aria-required': ariaRequired, 'data-testid': testid },
  ref,
) {
  const optionTestid = React.useCallback(
    (v: string) => (testid ? `${testid}-opt-${v}` : undefined),
    [testid],
  )
  const s = useSurface({ disabled })
  const listboxId = React.useId()
  const [open, setOpen] = React.useState(false)
  const [query, setQuery] = React.useState('')
  const [current, setCurrent] = useControllableState<string[]>({
    value, defaultValue: defaultValue ?? [], onChange: (v) => { onValueChange?.(v); onChange?.(v) },
  })
  const locked = s.disabled || loading || s.readOnly
  // O(1) lookups: label-by-value (tag render) + a selected Set (Check + toggle) — replaces the
  // per-item `.find()`/`.includes()` that made selection rendering O(n²).
  const labelByValue = React.useMemo(() => {
    const m = new Map<string, string>()
    for (const o of options) m.set(o.value, o.label)
    return m
  }, [options])
  // The per-option tag override, kept SEPARATE from `labelByValue` so the remove button's
  // accessible name (which needs a string) still comes from `label` while the tag renders a node.
  const tagByValue = React.useMemo(() => {
    const m = new Map<string, React.ReactNode>()
    for (const o of options) if (o.selectedLabel != null) m.set(o.value, o.selectedLabel)
    return m
  }, [options])
  const selectedSet = React.useMemo(() => new Set(current), [current])
  // De-dupe incoming value/defaultValue so repeated entries never collide as React keys on the
  // tags + hidden inputs (a parent may pass a list with duplicates).
  const uniqueCurrent = React.useMemo(() => [...selectedSet], [selectedSet])
  // functional updater (the hook supports it) → no stale read-modify-write on rapid toggles.
  // Mutations are no-ops while locked (disabled/loading/readOnly).
  const toggle = (v: string) => { if (locked) return; setCurrent((prev) => prev.includes(v) ? prev.filter((x) => x !== v) : [...prev, v]) }
  // Create-only (add, never toggle off) for free-text tokens.
  const addToken = (v: string) => { if (locked) return; const t = v.trim(); if (t) setCurrent((prev) => prev.includes(t) ? prev : [...prev, t]) }
  const resolvedCreateLabel = createLabel ?? ((q: string) => `Create "${q}"`)

  const trimmed = query.trim()
  const exists = options.some((o) => o.value === trimmed || o.label === trimmed) || selectedSet.has(trimmed)
  const canCreate = !!allowCreate && trimmed !== '' && !exists
  const handleInput = (raw: string) => {
    if (allowCreate && tokenSeparators.length) {
      const { tokens, rest } = splitOnSeparators(raw, tokenSeparators)
      tokens.forEach(addToken)
      setQuery(rest)
    } else {
      setQuery(raw)
    }
  }

  if (s.loading) return <Skeleton className={cn('h-8 w-full rounded-lg', className)} />
  return (
    <Root open={open} onOpenChange={(o) => { if (locked && o) return; setOpen(o); if (!o) { onBlur?.(); setQuery('') } }}>
      {/* native form submission: one hidden input per selected value (div trigger has no name). */}
      {name != null && uniqueCurrent.map((v) => <input key={v} type="hidden" name={name} value={v} />)}
      <PopoverTrigger nativeButton={false} render={
        /* a DIV (not a <button>) so the removable Tag <button>s can legally nest; keyboard
           open is wired manually since a div has no implicit button activation. Base UI must
           be told this isn't a native button (nativeButton=false) or it warns. */
        <div
          ref={ref}
          id={id}
          role="combobox"
          tabIndex={locked ? -1 : 0}
          aria-expanded={open}
          aria-haspopup="listbox"
          aria-controls={listboxId}
          aria-invalid={invalid || undefined}
          aria-busy={loading || undefined}
          aria-describedby={ariaDescribedby}
          aria-label={ariaLabel}
          aria-labelledby={ariaLabelledby}
          aria-required={ariaRequired || undefined}
          aria-disabled={locked || undefined}
          data-testid={testid}
          style={style}
          onKeyDown={(e) => {
            if (locked) return
            if (e.key === 'Enter' || e.key === ' ' || e.key === 'ArrowDown') { e.preventDefault(); setOpen(true) }
          }}
          className={cn(
            'flex min-h-8 w-full flex-wrap items-center gap-1 rounded-lg border border-input bg-transparent px-2 py-1 text-sm',
            'focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 outline-none',
            locked && 'cursor-not-allowed opacity-50',
            className, invalid && 'border-destructive focus-visible:ring-destructive',
          )}
        >
          {uniqueCurrent.length === 0 && <span className="px-1 text-muted-foreground">{placeholder}</span>}
          {uniqueCurrent.map((v) => {
            const label = labelByValue.get(v) ?? v
            return (
              // stop the REMOVE control's click/keys from bubbling to the trigger (which would
              // open it) — and ONLY the remove control's.
              //
              // These three handlers used to fire unconditionally, so the tag swallowed EVERY
              // gesture aimed at it, not just the one aimed at its `×`. Once a value is chosen
              // the tag IS most of what the trigger shows (it is `max-w-full`, and a caller's
              // `selectedLabel` may be two lines), so the pointer lands on the tag for anything
              // aimed at the middle of the control — and the list did not open. What was left
              // was the chevron and whatever padding the tags had not filled: a combobox that
              // stops responding to being clicked the moment it holds something is not a
              // smaller version of a working one.
              //
              // Measured through the real control: with one tag bound, a click at the centre of
              // the trigger left it FOCUSED and CLOSED (`aria-expanded` never flipped), which is
              // indistinguishable from a picker whose options are gone.
              <span className="min-w-0 max-w-full" key={v} onClick={stopFromTagClose} onPointerDown={stopFromTagClose} onKeyDown={stopFromTagClose}>
                {/* max-w-full caps the tag at the trigger's own width, so a long option can no
                    longer push the tag past the control's edge (the tag is `whitespace-nowrap`,
                    so without a cap its content simply grows). What gives inside that cap is the
                    caller's to decide — via `selectedLabel`, whose parts choose their own
                    truncate/wrap. */}
                <Tag className="min-w-0 max-w-full" data-testid={`${testid}-tag-${v}`} onClose={() => { if (!locked) toggle(v) }} closeLabel={removeLabel(label)}>
                  {tagByValue.get(v) ?? label}
                </Tag>
              </span>
            )
          })}
          <ChevronsUpDown className="ml-auto size-4 shrink-0 opacity-50" aria-hidden />
        </div>
      } />
      {/* The option list sizes to its widest row, never to the trigger — `optionListPopupWidth`
          is the ONE rule every option list in the kit wears. This popup had no knob at all
          before it, which is precisely why it needed to be a default and not a prop. */}
      <PopoverContent className={cn(optionListPopupWidth, 'p-0')} align="start">
        {virtual ? (
          <VirtualMultiList
            options={options} selectedSet={selectedSet} onToggle={toggle}
            searchPlaceholder={searchPlaceholder} emptyText={emptyText} listboxId={listboxId}
            allowCreate={allowCreate} tokenSeparators={tokenSeparators} onCreateToken={addToken} createLabel={resolvedCreateLabel}
            optionTestid={optionTestid}
          />
        ) : (
          <Command shouldFilter={!allowCreate}>
            <CommandInput placeholder={searchPlaceholder} value={query} onValueChange={handleInput} />
            <CommandList id={listboxId}>
              {!canCreate && <CommandEmpty>{emptyText}</CommandEmpty>}
              {canCreate && (
                <CommandGroup>
                  <CommandItem value={`__create__${trimmed}`} onSelect={() => { addToken(trimmed); setQuery('') }}>
                    <Plus className="mr-2 size-4" aria-hidden />
                    {resolvedCreateLabel(trimmed)}
                  </CommandItem>
                </CommandGroup>
              )}
              {/* with allowCreate, cmdk filtering is off → filter here so the list still narrows. */}
              {groupOptions(
                options.filter((o) => !allowCreate || trimmed === '' || o.label.toLowerCase().includes(trimmed.toLowerCase())),
              ).map((g, gi) => (
                <CommandGroup key={g.heading ?? `__ungrouped__${gi}`} heading={g.heading}>
                  {g.options.map((o) => (
                    <CommandItem key={o.value} value={o.value} keywords={[o.label]} disabled={o.disabled} onSelect={() => toggle(o.value)} data-testid={optionTestid(o.value)}>
                      <Check className={cn('mr-2 size-4', selectedSet.has(o.value) ? 'opacity-100' : 'opacity-0')} aria-hidden />
                      <span className="truncate" title={o.label}>{o.label}</span>
                    </CommandItem>
                  ))}
                </CommandGroup>
              ))}
            </CommandList>
          </Command>
        )}
      </PopoverContent>
    </Root>
  )
})
