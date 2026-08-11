import * as React from 'react'
import {
  Sheet as Root, SheetTrigger, SheetContent, SheetHeader, SheetFooter, SheetTitle, SheetDescription,
} from '../shadcn/sheet'
import { Spinner } from './spinner'
import { cn } from '../lib/utils'

type SheetBase = {
  open?: boolean
  onOpenChange?: (open: boolean) => void
  /** Accessible name — required (the dialog must be labelled). */
  title: React.ReactNode
  description?: React.ReactNode
  footer?: React.ReactNode
  side?: 'top' | 'right' | 'bottom' | 'left'
  trigger?: React.ReactElement
  /** Allow closing by clicking the backdrop (legacy `maskClosable`). Default true. */
  maskClosable?: boolean
  className?: string
  children?: React.ReactNode
  /** Test selector — forwarded onto the sheet content <root> (i18n-safe). */
  'data-testid': string
} & (
  // loading replaces the body with a spinner; its accessible name is REQUIRED then (i18n).
  | { loading?: false; loadingLabel?: never }
  | { loading: true; loadingLabel: string }
)
// Resizable drawer: a draggable + keyboard-operable edge handle. When enabled, `resizeLabel`
// (accessible name for the separator handle) is required.
export type SheetProps =
  | (SheetBase & { resizable?: false; resizeLabel?: never; defaultSize?: never; minSize?: never; maxSize?: never })
  | (SheetBase & {
      resizable: true
      resizeLabel: string
      /** Initial px size of the resizable axis (width for left/right, height for top/bottom). */
      defaultSize?: number
      minSize?: number
      maxSize?: number
    })

const clamp = (n: number, lo: number, hi: number) => Math.min(Math.max(n, lo), hi)

export function Sheet({ open, onOpenChange, title, description, footer, side = 'right', trigger, maskClosable = true, loading, loadingLabel, className, children, 'data-testid': testid, ...rest }: SheetProps) {
  const resizable = (rest as { resizable?: boolean }).resizable
  const resizeLabel = (rest as { resizeLabel?: string }).resizeLabel
  const minSize = (rest as { minSize?: number }).minSize ?? 280
  const maxSize = (rest as { maxSize?: number }).maxSize ?? 720
  const horizontal = side === 'left' || side === 'right' // resize width vs height
  const [size, setSize] = React.useState((rest as { defaultSize?: number }).defaultSize ?? 384)
  const drag = React.useRef<{ pos: number; size: number } | null>(null)

  const onPointerDown = (e: React.PointerEvent) => {
    drag.current = { pos: horizontal ? e.clientX : e.clientY, size }
    ;(e.currentTarget as HTMLElement).setPointerCapture(e.pointerId)
  }
  const onPointerMove = (e: React.PointerEvent) => {
    if (!drag.current) return
    const cur = horizontal ? e.clientX : e.clientY
    const raw = cur - drag.current.pos
    // grow direction depends on which edge the drawer is anchored to.
    const delta = side === 'right' || side === 'bottom' ? -raw : raw
    setSize(clamp(drag.current.size + delta, minSize, maxSize))
  }
  const onKeyDown = (e: React.KeyboardEvent) => {
    const dec = horizontal ? 'ArrowLeft' : 'ArrowUp'
    const inc = horizontal ? 'ArrowRight' : 'ArrowDown'
    if (e.key === dec) { e.preventDefault(); setSize((s) => clamp(s - 16, minSize, maxSize)) }
    else if (e.key === inc) { e.preventDefault(); setSize((s) => clamp(s + 16, minSize, maxSize)) }
  }

  // handle sits on the drawer's INNER edge.
  const handlePos =
    side === 'right' ? 'left-0 top-0 h-full w-1.5 cursor-ew-resize'
      : side === 'left' ? 'right-0 top-0 h-full w-1.5 cursor-ew-resize'
        : side === 'top' ? 'bottom-0 left-0 w-full h-1.5 cursor-ns-resize'
          : 'top-0 left-0 w-full h-1.5 cursor-ns-resize'

  // maskClosable=false → backdrop click no longer dismisses (Base UI Dialog
  // exposes this on the Root via `dismissible`; Escape still works).
  return (
    <Root
      open={open}
      onOpenChange={(o, details) => {
        // maskClosable=false: ignore outside/backdrop dismissals (Escape +
        // programmatic close still work). Base UI has no `dismissible` prop, so
        // we filter on the change reason instead.
        if (!o && maskClosable === false && (details as { reason?: string }).reason === 'outside-press') return
        onOpenChange?.(o)
      }}
    >
      {trigger != null && <SheetTrigger render={trigger} />}
      <SheetContent
        side={side}
        className={cn(resizable && 'max-w-none', className)}
        style={resizable ? (horizontal ? { width: size } : { height: size }) : undefined}
        data-testid={testid}
        {...(description == null ? { 'aria-describedby': undefined } : {})}
      >
        <SheetHeader>
          <SheetTitle>{title}</SheetTitle>
          {description != null && <SheetDescription>{description}</SheetDescription>}
        </SheetHeader>
        {/* No `min-h-0` here, and that is a measured decision rather than an oversight — the
            obvious companion to the `max-h-[85svh]` cap in `shadcn/sheet.tsx`, and it would do
            nothing. A flex item's automatic minimum size is its min-content size only while its
            computed `overflow` is `visible`; this element is the scroll container itself
            (`overflow-y-auto`), so its automatic minimum is already 0 and it shrinks without
            help. Measured on a 390×800 phone with 3000px of content and no `min-h-0`: body
            height 607, scrollHeight 3220 — a real track. Adding the class changed nothing, and a
            class the layout does not depend on is one more thing a test would certify falsely. */}
        <div className="flex-1 overflow-y-auto px-4">
          {/* min-h centers reliably even though SheetContent isn't a flex column. */}
          {loading
            ? <div className="flex min-h-40 items-center justify-center"><Spinner label={loadingLabel ?? ''} /></div>
            : children}
        </div>
        {footer != null && <SheetFooter>{footer}</SheetFooter>}
        {resizable && (
          <div
            role="separator"
            aria-orientation={horizontal ? 'vertical' : 'horizontal'}
            aria-label={resizeLabel}
            tabIndex={0}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={() => { drag.current = null }}
            onPointerCancel={() => { drag.current = null }}
            onKeyDown={onKeyDown}
            className={cn('absolute z-20 hover:bg-accent focus-visible:bg-accent focus-visible:outline-none', handlePos)}
          />
        )}
      </SheetContent>
    </Root>
  )
}
