import * as React from 'react'
import { Check, ChevronDownIcon, Loader2, Search, X } from 'lucide-react'
import {
  Select as SelectRoot,
  SelectTrigger,
  SelectValue,
  SelectContent,
  SelectItem,
  SelectGroup,
  SelectLabel,
  selectLabelClasses,
  selectTriggerClasses,
} from '../shadcn/select'
import { Popover, PopoverTrigger, PopoverContent } from '../shadcn/popover'
import { InputGroup, InputGroupAddon } from '../shadcn/input-group'
import { Skeleton } from '../shadcn/skeleton'
import { useSurface } from './surface'
import { useControllableState } from './use-controllable-state'
import { cn } from '../lib/utils'
import type { ValueBinding } from './value-binding'

/**
 * How many options a `Select` must offer before `showSearch: 'auto'` gives it a search field.
 *
 * TEN, and the number is read off the popup rather than picked: the list is capped at
 * `max-h-72` (288px) over ~32px rows, so NINE rows are on screen at once. At or below that the
 * whole set is visible and the eye is faster than the keyboard — a search box there is a second
 * thing to read for reach the user already has. The tenth option is the first one that can only
 * be reached by SCROLLING, which is exactly the point where typing becomes the shorter path.
 *
 * Exported because the threshold is a property callers and tests reason about; a literal `10`
 * repeated at each of those sites is how the two drift apart.
 */
export const SELECT_SEARCH_THRESHOLD = 10

export interface SelectOption {
  label: React.ReactNode
  value: string
  disabled?: boolean
  /** What a search query is matched against, when the visible `label` is not it — a node label
   *  has no text to search, and a caller may want a row findable by more than it displays.
   *  Defaults to a string `label`, and to `value` when the label is a node. */
  searchText?: string
  /** What to show in the TRIGGER when this option is selected, if it should differ from
   *  `label` (the dropdown-row content). Falls back to `label`. */
  selectedLabel?: React.ReactNode
}
export interface SelectOptionGroup {
  label?: React.ReactNode
  options: SelectOption[]
}

// Surface: region loading → skeleton · own loading → trigger spinner · disabled · readOnly · size.
// Form-bindable: value + onChange(value) (alias of onValueChange) + onBlur + name + id + ref(→trigger)
// + aria-* passthrough so FormField's aria-describedby/required reach the trigger.
// Custom render: `optionRender` controls each dropdown row; per-option `selectedLabel`
// (or `labelRender`) controls the selected display in the trigger — so the row and the
// selected value can render differently (legacy optionRender / labelRender / optionLabelProp).
interface SelectBase {
  options: (SelectOption | SelectOptionGroup)[]
  onBlur?: () => void
  placeholder?: string
  disabled?: boolean
  loading?: boolean
  invalid?: boolean
  size?: 'sm' | 'default' | 'lg'
  name?: string
  id?: string
  className?: string
  /** Custom content for each dropdown row. Receives the option. */
  optionRender?: (option: SelectOption) => React.ReactNode
  /** Custom content for the selected value in the trigger. Receives the selected option
   *  (undefined if none). Overrides per-option `selectedLabel`. */
  labelRender?: (option: SelectOption | undefined) => React.ReactNode
  /** Constrain the dropdown to the trigger's width (legacy `popupMatchSelectWidth`).
   *  Default true (exact match); false lets the dropdown grow wider for long options. */
  popupMatchSelectWidth?: boolean
  /** Search inside the popup. `'auto'` (the default) turns it on once the option count reaches
   *  {@link SELECT_SEARCH_THRESHOLD}; `true` / `false` force it either way.
   *
   *  Search changes the composition underneath (see {@link SearchableSelect}), never the
   *  contract: same trigger, same `data-testid`s, same option ids, same `optionRender` /
   *  `labelRender` / group headings. A caller cannot tell which arm it got except by typing. */
  showSearch?: boolean | 'auto'
  /** Placeholder for the search field. Optional with a default, unlike `MultiSelect`'s, because
   *  `'auto'` can turn search on for a caller that never asked for it — making the copy required
   *  would break every existing call site for behaviour it did not opt into. Pass your own
   *  wording (and translate it) wherever the control is user-facing. */
  searchPlaceholder?: string
  /** What the list reads as when the filter matches nothing. Same defaulting rationale as
   *  `searchPlaceholder`; an empty popup is indistinguishable from a broken one, so there is
   *  always SOME sentence here. */
  emptyText?: string
  'aria-describedby'?: string
  'aria-label'?: string
  'aria-labelledby'?: string
  'aria-required'?: boolean
  /** Test selector — forwarded onto <root> (i18n-safe). Options derive `${testid}-opt-${value}`. */
  'data-testid': string
}
// allowClear adds a clear button → its accessible name (clearLabel) is REQUIRED (no default, for i18n).
// Controlled `value` requires a change handler (see ValueBinding); FormField stays valid.
export type SelectProps = SelectBase &
  ValueBinding<string> &
  ({ allowClear?: false; clearLabel?: never } | { allowClear: true; clearLabel: string })

const isGroup = (o: SelectOption | SelectOptionGroup): o is SelectOptionGroup =>
  Array.isArray((o as SelectOptionGroup).options)

function flatOptions(options: (SelectOption | SelectOptionGroup)[]): SelectOption[] {
  return options.flatMap((o) => (isGroup(o) ? o.options : [o]))
}

// ── SEARCH ───────────────────────────────────────────────────────────────────────────────
//
// Past a certain length a listbox is not a choice, it is a scroll. `MultiSelect` had the same
// problem and answered it TWICE — a cmdk `Command` for ordinary lists, and `VirtualMultiList`
// (its own search field, its own arrow-key nav, its own `aria-activedescendant`) for the large
// ones. This mirrors the SECOND, minus the virtualizer, and the choice is not stylistic:
//
//   · cmdk re-picks its own first row whenever the query changes, against the list as it stood
//     BEFORE our narrowing, and it recomputes `aria-activedescendant` only when the value moves
//     through its own store. Filtering ourselves (which we must, to drop an emptied GROUP rather
//     than merely hide it, and to match on the heading) therefore leaves a cursor pointing at a
//     row that is unmounting and an active row nobody announces. Both were measured, not feared.
//   · Base UI's Select popup cannot host a text field at all: it owns focus and treats keystrokes
//     as typeahead over its own list. So the searchable arm has to be a Popover either way.
//   · The kit's `Combobox` (Base UI Combobox) is not this control: string-only labels, no group
//     headings at all, no `labelRender`/`optionRender`, and an `<input>` as its resting state —
//     so the CHOSEN value can only be a string in a text field. Callers that render a chosen
//     value as anything richer (or that group by anything) cannot express it there.
//   · Keeping ONE component means callers do not choose between two pickers, and the DOM
//     contract — trigger role, `data-testid`, `${testid}-opt-${value}` rows, group headings —
//     is identical on both arms by construction rather than by two authors agreeing.
//
// NOT virtualized. The base-ui arm renders every option today, so an unwindowed filtered list is
// no worse than what it replaces, and `max-h-72` + the filter bound what is on screen. Windowing
// is the next step if a real list makes it one, not a speculative one taken here.

/** The text a query is matched against for one option. A node label has nothing to read, so an
 *  explicit `searchText` wins and `value` is the last resort (a row must stay REACHABLE by
 *  typing even when its label is a component). */
function optionHaystack(o: SelectOption): string {
  if (o.searchText != null) return o.searchText
  return typeof o.label === 'string' ? o.label : o.value
}

const headingText = (label: React.ReactNode): string => (typeof label === 'string' ? label : '')

/** `options` as heading-bearing RUNS in declaration order — consecutive ungrouped options
 *  collapse into one anonymous run, so an ungrouped list renders exactly one group. Builds new
 *  objects; the caller's arrays are never mutated. */
function optionRuns(options: (SelectOption | SelectOptionGroup)[]): SelectOptionGroup[] {
  const out: SelectOptionGroup[] = []
  for (const o of options) {
    if (isGroup(o)) {
      out.push({ label: o.label, options: [...o.options] })
      continue
    }
    const last = out[out.length - 1]
    if (last && last.label == null) last.options.push(o)
    else out.push({ options: [o] })
  }
  return out
}

/**
 * Narrow the runs to what matches `query`.
 *
 * Two properties, both of them the reason this is a function and not a `.filter()` inline:
 *
 *   · a run whose every option went is DROPPED, heading and all. A heading left standing over
 *     nothing says a group has a match when it has none, which inverts what the heading was
 *     added to tell the user.
 *   · a run whose HEADING matches keeps all of its options. The heading is frequently the only
 *     thing that distinguishes two identically-labelled rows (two datasets' `expression`), so a
 *     search that could not reach it would narrow such a list to itself and answer nothing.
 */
function filterRuns(runs: SelectOptionGroup[], query: string): SelectOptionGroup[] {
  const q = query.trim().toLowerCase()
  if (q === '') return runs
  const out: SelectOptionGroup[] = []
  for (const g of runs) {
    const kept = headingText(g.label).toLowerCase().includes(q)
      ? g.options
      : g.options.filter((o) => optionHaystack(o).toLowerCase().includes(q))
    if (kept.length > 0) out.push({ label: g.label, options: kept })
  }
  return out
}

// Fallback copy. `showSearch` defaults to `'auto'`, so search can appear for a caller that never
// passed either string — there has to be SOMETHING here, and an empty popup with no sentence in
// it is the illegible state this arm exists to avoid. User-facing surfaces pass their own.
const DEFAULT_SEARCH_PLACEHOLDER = 'Search…'
const DEFAULT_SEARCH_LABEL = 'Search options'
const DEFAULT_EMPTY_TEXT = 'No match'

interface SearchableSelectProps {
  options: (SelectOption | SelectOptionGroup)[]
  current: string
  onCommit: (value: string) => void
  onBlur?: () => void
  placeholder?: string
  searchPlaceholder?: string
  emptyText?: string
  locked: boolean
  loading?: boolean
  invalid?: boolean
  size: 'sm' | 'default'
  name?: string
  id?: string
  className?: string
  popupMatchSelectWidth: boolean
  optionRender?: (option: SelectOption) => React.ReactNode
  display: React.ReactNode
  testid: string
  clear?: { label: string }
  aria: {
    describedby?: string
    label?: string
    labelledby?: string
    required?: boolean
  }
}

/** The searchable arm. Same trigger, same ids, same rendering hooks — a filtered list instead of
 *  a scrolled one. Split out (rather than branching inside `Select`) so the base-ui arm keeps
 *  its exact hook order and stays byte-identical for every caller below the threshold. */
const SearchableSelect = React.forwardRef<HTMLButtonElement, SearchableSelectProps>(
  function SearchableSelect(
    {
      options, current, onCommit, onBlur, placeholder, searchPlaceholder, emptyText, locked, loading,
      invalid, size, name, id, className, popupMatchSelectWidth, optionRender, display, testid, clear, aria,
    },
    ref,
  ) {
    const [open, setOpen] = React.useState(false)
    const [query, setQuery] = React.useState('')
    const listboxId = React.useId()
    const inputRef = React.useRef<HTMLInputElement>(null)
    const runs = React.useMemo(() => filterRuns(optionRuns(options), query), [options, query])

    // The KEYBOARD CURSOR, held here and CLAMPED to what is currently offered.
    //
    // DERIVED, not stored-and-corrected: a cursor that no longer names a visible row IS the
    // first visible row, at every render. Filtering and moving therefore cannot race — there is
    // no window in which the cursor points at a row that has been filtered away, which is
    // exactly the window in which an ArrowDown starts from nowhere and Enter commits a row the
    // user never moved to.
    const [cursor, setCursor] = React.useState('')
    const visible = React.useMemo(
      () => runs.flatMap((g) => g.options).filter((o) => !o.disabled).map((o) => o.value),
      [runs],
    )
    const active = visible.includes(cursor) ? cursor : (visible[0] ?? '')
    const optionDomId = (v: string): string => `${listboxId}-opt-${v}`
    // Focus the field the moment the popup exists, so typing is what OPENING the control does —
    // a user who has to click the field first has a search box, not a searchable control.
    React.useEffect(() => {
      if (open) inputRef.current?.focus()
    }, [open])
    // Keep the cursor in view as it moves. Optional-called: this is a layout API, and the arms
    // are exercised in environments that have no layout at all.
    React.useEffect(() => {
      if (open && active !== '') {
        // `ownerDocument`, never the ambient `document`: in a popped-out window this subtree lives
        // in ANOTHER document, where a global lookup finds nothing (or, worse, a same-id node in
        // the opener). See kit/portal-container.tsx.
        const doc = inputRef.current?.ownerDocument ?? (typeof document === 'undefined' ? null : document)
        doc?.getElementById(optionDomId(active))?.scrollIntoView?.({ block: 'nearest' })
      }
      // `optionDomId` closes over `listboxId`, which is stable for the component's lifetime.
      // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [open, active])
    const commit = (v: string): void => {
      onCommit(v)
      setOpen(false)
      setQuery('')
    }
    const step = (delta: 1 | -1): void => {
      if (visible.length === 0) return
      const at = visible.indexOf(active)
      const next = at < 0 ? 0 : Math.min(visible.length - 1, Math.max(0, at + delta))
      setCursor(visible[next])
    }
    const onSearchKeyDown = (e: React.KeyboardEvent): void => {
      if (e.key === 'ArrowDown') { e.preventDefault(); step(1) }
      else if (e.key === 'ArrowUp') { e.preventDefault(); step(-1) }
      else if (e.key === 'Home') { e.preventDefault(); if (visible.length) setCursor(visible[0]) }
      else if (e.key === 'End') { e.preventDefault(); if (visible.length) setCursor(visible[visible.length - 1]) }
      else if (e.key === 'Enter') {
        e.preventDefault()
        // Nothing offered ⇒ nothing committed. Enter on a query that matches no row must not
        // fall back to "the last thing that was highlighted".
        if (active !== '') commit(active)
      }
    }
    return (
      <Popover
        open={open}
        onOpenChange={(o: boolean) => {
          if (locked && o) return
          setOpen(o)
          if (!o) {
            setQuery('')
            onBlur?.()
          }
        }}
      >
        {/* native form submission: the trigger is a <button>, which carries no value. */}
        {name != null && <input type="hidden" name={name} value={current} />}
        {/* relative wrapper so the clear button is a SIBLING of the trigger, never a <button>
            nested inside the trigger <button> (invalid HTML + keyboard-unreachable). */}
        <div className="relative">
          <PopoverTrigger
            render={
              <button
                ref={ref}
                type="button"
                id={id}
                role="combobox"
                aria-haspopup="listbox"
                aria-expanded={open}
                aria-controls={listboxId}
                aria-invalid={invalid || undefined}
                aria-busy={loading || undefined}
                aria-describedby={aria.describedby}
                aria-label={aria.label}
                aria-labelledby={aria.labelledby}
                aria-required={aria.required}
                disabled={locked}
                data-testid={testid}
                data-slot="select-trigger"
                data-size={size}
                data-placeholder={current === '' ? '' : undefined}
                className={cn(selectTriggerClasses, 'w-full', className, clear && 'pe-14')}
              >
                <span data-slot="select-value" className="flex flex-1 text-start">
                  {display ?? placeholder}
                </span>
                {loading && <Loader2 className="ms-2 size-4 shrink-0 animate-spin opacity-70" aria-hidden />}
                <ChevronDownIcon className="pointer-events-none size-4 text-muted-foreground" aria-hidden />
              </button>
            }
          />
          {clear && (
            <button
              type="button"
              aria-label={clear.label}
              data-testid={`${testid}-clear`}
              onClick={() => onCommit('')}
              // pointer-down stop so clearing via mouse doesn't also open the popup.
              onPointerDown={(e) => { e.preventDefault(); e.stopPropagation() }}
              className="absolute end-7 top-1/2 inline-flex -translate-y-1/2 items-center justify-center rounded-sm opacity-60 hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
            >
              <X className="size-3.5" aria-hidden />
            </button>
          )}
        </div>
        <PopoverContent
          data-testid={`${testid}-popup`}
          align="start"
          className={cn(
            'p-0',
            popupMatchSelectWidth ? 'w-(--anchor-width)' : 'w-auto min-w-(--anchor-width)',
          )}
        >
          {/* The search FIELD, wearing the same chrome as `MultiSelect`'s. */}
          <div className="p-1 pb-0">
            <InputGroup className="h-8! rounded-lg! border-input/30 bg-input/30 shadow-none! *:data-[slot=input-group-addon]:ps-2!">
              <input
                ref={inputRef}
                type="text"
                role="combobox"
                aria-expanded
                aria-controls={listboxId}
                aria-autocomplete="list"
                // The row the keyboard is ON, named for a screen reader. It is stated at every
                // render — including the render right after a keystroke narrows the list — so a
                // user who filters is told what they have landed on rather than left to guess.
                aria-activedescendant={active !== '' ? optionDomId(active) : undefined}
                // A placeholder is not an accessible name.
                aria-label={searchPlaceholder ?? DEFAULT_SEARCH_LABEL}
                data-testid={`${testid}-search`}
                placeholder={searchPlaceholder ?? DEFAULT_SEARCH_PLACEHOLDER}
                autoComplete="off"
                spellCheck={false}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={onSearchKeyDown}
                className="w-full bg-transparent text-sm outline-hidden placeholder:text-muted-foreground"
              />
              <InputGroupAddon>
                <Search className="size-4 shrink-0 opacity-50" aria-hidden />
              </InputGroupAddon>
            </InputGroup>
          </div>
          {runs.length === 0 ? (
            // Deliberately NOT a `-empty` testid: that suffix already names "there was nothing
            // to choose from", which is a DIFFERENT state from "your filter matched nothing",
            // and callers query for it by suffix.
            <div
              data-testid={`${testid}-no-match`}
              className="py-6 text-center text-sm text-muted-foreground"
            >
              {emptyText ?? DEFAULT_EMPTY_TEXT}
            </div>
          ) : (
            <div role="listbox" id={listboxId} className="max-h-72 overflow-y-auto p-1">
              {runs.map((g, gi) => (
                <div key={headingText(g.label) || `__ungrouped__${gi}`} role="group">
                  {/* The heading wears the SAME slot + classes the base-ui arm's `SelectLabel`
                      does, so a reader (or a test) cannot tell the two arms apart by it. A run
                      with nothing left in it never reaches here — see `filterRuns`. */}
                  {g.label != null && <div data-slot="select-label" className={selectLabelClasses}>{g.label}</div>}
                  {g.options.map((o) => (
                    <div
                      key={o.value}
                      id={optionDomId(o.value)}
                      role="option"
                      // CHOSEN, not active: the cursor is carried by `aria-activedescendant`.
                      aria-selected={o.value === current}
                      aria-disabled={o.disabled || undefined}
                      data-testid={`${testid}-opt-${o.value}`}
                      onMouseMove={() => { if (!o.disabled) setCursor(o.value) }}
                      onClick={() => { if (!o.disabled) commit(o.value) }}
                      className={cn(
                        'relative flex w-full cursor-default items-center gap-1.5 rounded-md py-1 pe-8 ps-1.5 text-sm select-none',
                        o.value === active && 'bg-accent text-accent-foreground',
                        o.disabled && 'pointer-events-none opacity-50',
                      )}
                    >
                      {optionRender ? optionRender(o) : o.label}
                      {o.value === current && (
                        <span className="pointer-events-none absolute end-2 flex size-4 items-center justify-center">
                          <Check className="size-4" aria-hidden />
                        </span>
                      )}
                    </div>
                  ))}
                </div>
              ))}
            </div>
          )}
        </PopoverContent>
      </Popover>
    )
  },
)

export const Select = React.forwardRef<HTMLButtonElement, SelectProps>(function Select(
  {
    options, value, defaultValue, onValueChange, onChange, onBlur, placeholder,
    disabled, loading, invalid, size, name, id, className, optionRender, labelRender, popupMatchSelectWidth = true,
    showSearch = 'auto', searchPlaceholder, emptyText,
    'aria-describedby': ariaDescribedby, 'aria-label': ariaLabel,
    'aria-labelledby': ariaLabelledby, 'aria-required': ariaRequired,
    'data-testid': testid,
    ...rest
  },
  ref,
) {
  const allowClear = (rest as { allowClear?: boolean }).allowClear
  const clearLabel = (rest as { clearLabel?: string }).clearLabel
  const s = useSurface({ disabled, size })
  // Single source of truth (drives Radix via value={current} below), so clear + custom display +
  // selection never desync. '' = no selection (kept a string so Radix stays controlled).
  const [current, setCurrent] = useControllableState<string>({
    value, defaultValue: defaultValue ?? '', onChange: (v) => { onValueChange?.(v); onChange?.(v) },
  })
  const handleChange = (v: string) => setCurrent(v)
  const clear = () => setCurrent('')
  // Radix Select has no readOnly — map an ambient readOnly surface to "can't change".
  const locked = s.disabled || loading || s.readOnly

  const items = React.useMemo(
    () =>
      options.map((o, i) =>
        isGroup(o) ? (
          <SelectGroup key={o.options[0]?.value ?? `g${i}`} className="p-0">
            {/* p-0 cancels base-nova's group inset (the popup now owns p-1). */}
            {o.label != null && <SelectLabel>{o.label}</SelectLabel>}
            {o.options.map((opt) => (
              <SelectItem
                key={opt.value}
                value={opt.value}
                disabled={opt.disabled}
                data-testid={testid ? `${testid}-opt-${opt.value}` : undefined}
              >
                {optionRender ? optionRender(opt) : opt.label}
              </SelectItem>
            ))}
          </SelectGroup>
        ) : (
          <SelectItem
            key={o.value}
            value={o.value}
            disabled={o.disabled}
            data-testid={testid ? `${testid}-opt-${o.value}` : undefined}
          >
            {optionRender ? optionRender(o) : o.label}
          </SelectItem>
        ),
      ),
    [options, optionRender, testid],
  )

  // O(1) selected-option lookup (replaces a per-render flatOptions().find()).
  const optionByValue = React.useMemo(() => {
    const m = new Map<string, SelectOption>()
    for (const o of flatOptions(options)) m.set(o.value, o)
    return m
  }, [options])

  if (s.loading) {
    return <Skeleton className={cn('h-9 w-full rounded-md', className)} />
  }

  // Custom selected display: a per-option selectedLabel or a labelRender means the trigger
  // must show something other than the row text → render controlled SelectValue children.
  const selectedOpt = current ? optionByValue.get(current) : undefined
  // base-ui's SelectValue renders the raw VALUE when given no children, so a plain
  // option would show its value ("light") instead of its label ("Light"). Always
  // drive the trigger from the option's label (or an explicit override).
  const customDisplay =
    labelRender != null
      ? labelRender(selectedOpt)
      : selectedOpt?.selectedLabel != null
        ? selectedOpt.selectedLabel
        : selectedOpt?.label
  const showClear = allowClear && current !== '' && !locked

  // Which arm. `'auto'` counts the options the caller actually passed (a prepended out-of-set
  // value included — it is a row the user scrolls past like any other), so the decision is made
  // on what is on screen rather than on what a caller declared somewhere upstream.
  const searchable =
    showSearch === 'auto' ? flatOptions(options).length >= SELECT_SEARCH_THRESHOLD : showSearch === true
  if (searchable) {
    return (
      <SearchableSelect
        ref={ref}
        options={options}
        current={current}
        onCommit={handleChange}
        onBlur={onBlur}
        placeholder={placeholder}
        searchPlaceholder={searchPlaceholder}
        emptyText={emptyText}
        locked={!!locked}
        loading={loading}
        invalid={invalid}
        size={s.size === 'sm' ? 'sm' : 'default'}
        name={name}
        id={id}
        className={className}
        popupMatchSelectWidth={popupMatchSelectWidth}
        optionRender={optionRender}
        // The SAME selected display the base-ui arm computes — one derivation, so a caller's
        // `labelRender` / `selectedLabel` cannot read one way below the threshold and another
        // above it.
        display={customDisplay}
        testid={testid}
        clear={showClear && clearLabel != null ? { label: clearLabel } : undefined}
        aria={{
          describedby: ariaDescribedby,
          label: ariaLabel,
          labelledby: ariaLabelledby,
          required: ariaRequired,
        }}
      />
    )
  }

  return (
    <SelectRoot value={current} onValueChange={(v) => handleChange(v ?? '')} disabled={locked} name={name}>
      {/* relative wrapper so the clear button is a SIBLING of the trigger, never a <button>
          nested inside the trigger <button> (invalid HTML + keyboard-unreachable). */}
      <div className="relative">
        <SelectTrigger
          ref={ref}
          id={id}
          onBlur={onBlur}
          aria-invalid={invalid || undefined}
          aria-busy={loading || undefined}
          aria-describedby={ariaDescribedby}
          aria-label={ariaLabel}
          aria-labelledby={ariaLabelledby}
          aria-required={ariaRequired}
          data-testid={testid}
          size={s.size === 'sm' ? 'sm' : 'default'}
          className={cn('w-full', className, showClear && 'pr-14')}
        >
          <SelectValue placeholder={placeholder}>{customDisplay}</SelectValue>
          {loading && <Loader2 className="ml-2 size-4 shrink-0 animate-spin opacity-70" aria-hidden />}
        </SelectTrigger>
        {showClear && (
          <button
            type="button"
            aria-label={clearLabel}
            data-testid={testid ? `${testid}-clear` : undefined}
            onClick={clear}
            // pointer-down stop so clearing via mouse doesn't also open the Radix dropdown.
            onPointerDown={(e) => { e.preventDefault(); e.stopPropagation() }}
            className="absolute right-7 top-1/2 inline-flex -translate-y-1/2 items-center justify-center rounded-sm opacity-60 hover:opacity-100 focus-visible:outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
          >
            <X className="size-3.5" aria-hidden />
          </button>
        )}
      </div>
      {/* p-1: base-nova puts the list inset on SelectGroup only, so flat
          (ungrouped) items would sit flush against the popup edge — restore it
          on the popup. match=false → let the popup grow past the trigger width
          (Base UI defaults the popup to the trigger's `--anchor-width`). */}
      <SelectContent
        data-testid={testid ? `${testid}-popup` : undefined}
        className={cn('p-1', !popupMatchSelectWidth && 'w-auto min-w-(--anchor-width)')}
      >
        {items}
      </SelectContent>
    </SelectRoot>
  )
})
