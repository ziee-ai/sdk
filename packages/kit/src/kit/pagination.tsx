import * as React from 'react'
import { ChevronLeft, ChevronRight } from 'lucide-react'
import {
  Pagination as Base, PaginationContent, PaginationItem, PaginationLink,
} from '../shadcn/pagination'
import { Select } from './select'
import { Input } from './input'
import { ClampNotice } from '../internal/clamp-notice'
import { cn } from '../lib/utils'

// Pagination (common subset): total/pageSize/current + onChange(page). All accessible names
// are REQUIRED (no defaults — caller owns the strings for i18n). Optional page-size changer
// and quick-jumper (each forces its own a11y label via the union below).
interface PaginationBase {
  total: number
  pageSize?: number
  current: number
  onChange: (page: number) => void
  /** Max numbered links to show (window around current); ends always shown. */
  siblingCount?: number
  className?: string
  /** Accessible name for the navigation landmark. */
  'aria-label': string
  /** Accessible name for the previous-page control. */
  previousLabel: string
  /** Accessible name for the next-page control. */
  nextLabel: string
  /** Builds the accessible name for a numbered page link, e.g. (p) => `Page ${p}`. */
  pageLabel: (page: number) => string
  /** Optional total-summary renderer, e.g. (total,[from,to]) => `${from}-${to} of ${total}`. */
  showTotal?: (total: number, range: [number, number]) => React.ReactNode
  /** Test selector — forwarded onto the nav <root> (i18n-safe). */
  'data-testid': string
}

// Page-size dropdown — when enabled, its accessible name + change handler are required.
type SizeChanger =
  | { showSizeChanger?: false; pageSizeOptions?: never; pageSizeLabel?: never; onPageSizeChange?: never }
  | {
      showSizeChanger: true
      /** Selectable page sizes (default [10, 20, 50]). */
      pageSizeOptions?: number[]
      /** Accessible name for the page-size select. */
      pageSizeLabel: string
      /** Fires with the new size. The parent should also reset `current` to 1 (component
       *  clamps display defensively, but the canonical page should be reset). */
      onPageSizeChange: (size: number) => void
    }

// Jump-to-page input — when enabled, its accessible name is required.
type QuickJumper =
  | { showQuickJumper?: false; jumpLabel?: never }
  | { showQuickJumper: true; jumpLabel: string }

export type PaginationProps = PaginationBase & SizeChanger & QuickJumper

function pageRange(current: number, pageCount: number, sibling: number): (number | 'gap')[] {
  const pages = new Set<number>([1, pageCount])
  for (let p = current - sibling; p <= current + sibling; p++) if (p >= 1 && p <= pageCount) pages.add(p)
  const sorted = [...pages].sort((a, b) => a - b)
  const out: (number | 'gap')[] = []
  let prev = 0
  for (const p of sorted) {
    if (prev && p - prev === 2) out.push(prev + 1) // exactly one hidden page → show it, not "…"
    else if (prev && p - prev > 1) out.push('gap')
    out.push(p)
    prev = p
  }
  return out
}

export function Pagination({
  total, pageSize = 10, current, onChange, siblingCount = 1, className,
  'aria-label': ariaLabel, previousLabel, nextLabel, pageLabel, showTotal,
  showSizeChanger, pageSizeOptions, pageSizeLabel, onPageSizeChange,
  showQuickJumper, jumpLabel, 'data-testid': testid,
}: PaginationProps) {
  const pageCount = Math.max(1, Math.ceil(total / pageSize))
  // Defensive clamp: a parent that changes pageSize without resetting `current` can leave it
  // out of range — clamp for display so we never render a nonsensical range/active page.
  const safeCurrent = Math.min(Math.max(current, 1), pageCount)
  const go = (p: number) => { if (p >= 1 && p <= pageCount && p !== current) onChange(p) }
  const [jump, setJump] = React.useState('')
  const [jumpNotice, setJumpNotice] = React.useState<string | null>(null)
  // Ensure the current pageSize is always a selectable option (else the Select trigger is blank).
  // Memoized so the derived options array is stable → the child Select's items memo isn't busted
  // on every pagination render. (Must run before the early return — Rules of Hooks.)
  const sizeItems = React.useMemo(() => {
    const base = pageSizeOptions ?? [10, 20, 50]
    const sizes = base.includes(pageSize) ? base : [...base, pageSize].sort((a, b) => a - b)
    return sizes.map((n) => ({ label: String(n), value: String(n) }))
  }, [pageSizeOptions, pageSize])
  if (pageCount <= 1 && showTotal == null && !showSizeChanger) return null
  const atStart = safeCurrent <= 1
  const atEnd = safeCurrent >= pageCount
  const from = total === 0 ? 0 : (safeCurrent - 1) * pageSize + 1
  const to = Math.min(safeCurrent * pageSize, total)
  // THE JUMP BOX USED TO ANSWER THREE OUTCOMES WITH THE SAME EMPTY BOX. Page 999 of 12 clamped
  // to 12; "3.5" was discarded entirely; a valid entry worked — and `setJump('')` cleared the
  // field in all three, so nothing on screen told them apart. A user who lands on page 12 after
  // asking for 999 has no way to know whether they mistyped, whether the control ignored them, or
  // whether there simply is no page 999. (internal/clamp-notice.tsx rules 2 and 3.)
  const submitJump = () => {
    const raw = jump.trim()
    const n = Number(raw)
    if (raw === '') { setJump(''); setJumpNotice(null); return }
    if (!Number.isInteger(n)) {
      setJumpNotice(`\u201c${raw}\u201d is not a page number`)
      setJump('')
      return
    }
    const target = Math.min(Math.max(n, 1), pageCount)
    setJumpNotice(
      target !== n
        ? `There ${pageCount === 1 ? 'is' : 'are'} only ${pageCount} page${pageCount === 1 ? '' : 's'} \u2014 went to ${target}`
        : null,
    )
    go(target)
    setJump('')
  }
  return (
    <Base className={cn('flex w-full items-center justify-end gap-3', className)} aria-label={ariaLabel} data-testid={testid}>
      {showTotal != null && <span className="text-sm text-muted-foreground">{showTotal(total, [from, to])}</span>}
      {pageCount > 1 && (
        <PaginationContent>
          <PaginationItem>
            <PaginationLink
              href="#"
              aria-label={previousLabel}
              aria-disabled={atStart}
              tabIndex={atStart ? -1 : undefined}
              className={atStart ? 'pointer-events-none opacity-50' : undefined}
              onClick={(e) => { e.preventDefault(); go(safeCurrent - 1) }}
            >
              <ChevronLeft className="size-4" aria-hidden />
            </PaginationLink>
          </PaginationItem>
          {pageRange(safeCurrent, pageCount, siblingCount).map((p, i) =>
            p === 'gap' ? (
              <PaginationItem key={`gap-${i}`}>
                <span aria-hidden className="flex size-9 items-center justify-center">…</span>
              </PaginationItem>
            ) : (
              <PaginationItem key={p}>
                <PaginationLink
                  href="#"
                  data-testid={`${testid}-page-${p}`}
                  isActive={p === safeCurrent}
                  aria-label={pageLabel(p)}
                  aria-current={p === safeCurrent ? 'page' : undefined}
                  onClick={(e) => { e.preventDefault(); go(p) }}
                >
                  {p}
                </PaginationLink>
              </PaginationItem>
            ),
          )}
          <PaginationItem>
            <PaginationLink
              href="#"
              aria-label={nextLabel}
              aria-disabled={atEnd}
              tabIndex={atEnd ? -1 : undefined}
              className={atEnd ? 'pointer-events-none opacity-50' : undefined}
              onClick={(e) => { e.preventDefault(); go(safeCurrent + 1) }}
            >
              <ChevronRight className="size-4" aria-hidden />
            </PaginationLink>
          </PaginationItem>
        </PaginationContent>
      )}
      {showSizeChanger && (
        <Select
          size="sm"
          data-testid={`${testid}-page-size`}
          aria-label={pageSizeLabel}
          value={String(pageSize)}
          options={sizeItems}
          onChange={(v) => onPageSizeChange(Number(v))}
        />
      )}
      {showQuickJumper && pageCount > 1 && (
        <Input
          size="sm"
          data-testid={`${testid}-jump`}
          className="w-16"
          inputMode="numeric"
          aria-label={jumpLabel}
          value={jump}
          onChange={(e) => { setJumpNotice(null); setJump(e.target.value) }}
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); submitJump() } }}
          onBlur={submitJump}
        />
      )}
      {showQuickJumper && pageCount > 1 && <ClampNotice message={jumpNotice} />}
    </Base>
  )
}
