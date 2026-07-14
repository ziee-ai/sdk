import * as React from 'react'
import * as DialogPrimitive from '@radix-ui/react-dialog'
import { Button, Title } from '@ziee/kit'
import { cn } from '@ziee/kit/lib/utils'
import { IoIosArrowBack } from 'react-icons/io'
import { ResizeHandle } from './ResizeHandle'
import { DivScrollY } from './DivScrollY'
import { useWindowMinSize } from '../hooks/useWindowMinSize'

type Placement = 'left' | 'right' | 'top' | 'bottom'

// App-wide Drawer primitive. Public API preserved from the previous antd-backed
// version (open/onClose/title/placement/size/footer/mask/extra/styles/classNames/...);
// internals run on Radix Dialog (the same primitive the kit Sheet uses), so
// consumers stay unchanged. Custom header (back button), edge ResizeHandle
// and the DivScrollY body layer are retained.
export interface DrawerProps {
  open?: boolean
  onClose?: () => void
  title?: React.ReactNode
  /**
   * Accessible name for the dialog when `title` is a non-string node (Radix
   * requires a `Dialog.Title`; a node can't be introspected for its text).
   * Ignored for string titles (the visible heading is used directly).
   */
  titleText?: string
  placement?: Placement
  /** Panel size on the resize axis: a px number, or legacy 'default'(378)/'large'(736). */
  size?: number | 'default' | 'large'
  /** Explicit width (overrides `size`); legacy escape hatch. */
  width?: number | string
  footer?: React.ReactNode
  extra?: React.ReactNode
  /** Backdrop. `false` = no overlay (non-modal); `{ closable:false }` = don't close on backdrop click. */
  mask?: boolean | { closable?: boolean }
  /** Close on backdrop click (legacy `maskClosable`). Overrides `mask.closable`. */
  maskClosable?: boolean
  /** Show the header close affordance (legacy `closable`). Default true. */
  closable?: boolean
  className?: string
  classNames?: { body?: string; header?: string; footer?: string; wrapper?: string }
  styles?: {
    body?: React.CSSProperties
    header?: React.CSSProperties
    footer?: React.CSSProperties
    wrapper?: React.CSSProperties
  }
  /** Unmount children when closed (legacy `destroyOnHidden`). Default true (Radix unmounts on close). */
  destroyOnHidden?: boolean
  zIndex?: number
  /** Render children directly (caller owns scrolling) instead of inside the DivScrollY layer. */
  noBodyScrollWrap?: boolean
  /** Overrides the default `layout-drawer-content` testid on the content root. */
  'data-testid'?: string
  children?: React.ReactNode
}

const sizePx = (size: DrawerProps['size']): number =>
  size === 'default' ? 378 : size === 'large' ? 736 : typeof size === 'number' ? size : 520

const sidePos: Record<Placement, string> = {
  // No `h-full`: with the floating-card `m-2` margin, height:100% (=100vh) plus the
  // 8px top margin pushes the bottom 8px+ off-screen. `inset-y-0` (top:0 + bottom:0)
  // with height:auto stretches to fill BETWEEN the insets, honoring the margins.
  right: 'inset-y-0 right-0 data-[state=open]:slide-in-from-right-10 data-[state=closed]:slide-out-to-right-10',
  left: 'inset-y-0 left-0 data-[state=open]:slide-in-from-left-10 data-[state=closed]:slide-out-to-left-10',
  top: 'inset-x-0 top-0 w-full data-[state=open]:slide-in-from-top-10 data-[state=closed]:slide-out-to-top-10',
  bottom: 'inset-x-0 bottom-0 w-full data-[state=open]:slide-in-from-bottom-10 data-[state=closed]:slide-out-to-bottom-10',
}

export const Drawer: React.FC<DrawerProps> = ({
  open,
  onClose,
  title,
  titleText,
  placement = 'right',
  size,
  width,
  footer,
  extra,
  mask = true,
  maskClosable: maskClosableProp,
  closable = true,
  className,
  classNames,
  styles,
  zIndex,
  noBodyScrollWrap = false,
  'data-testid': testid,
  children,
}) => {
  const windowMinSize = useWindowMinSize()
  const horizontal = placement === 'left' || placement === 'right'

  // Touch swipe-to-close: drag the panel toward the edge it's docked to (a right
  // drawer → swipe right) and release past a threshold to close; otherwise it
  // snaps back. Follows the finger live. Horizontal placements only.
  const contentRef = React.useRef<HTMLDivElement>(null)
  const swipe = React.useRef<{
    x: number
    y: number
    active: boolean
    dx: number
  } | null>(null)
  const closeDir = placement === 'right' ? 1 : placement === 'left' ? -1 : 0
  const onTouchStart = (e: React.TouchEvent) => {
    if (closeDir === 0 || e.touches.length !== 1) return
    // Don't hijack a horizontal scroller inside the drawer (e.g. an xlsx sheet-
    // tab strip): if the touch starts in one, let it scroll instead of closing.
    for (
      let el = e.target as HTMLElement | null;
      el && el !== e.currentTarget;
      el = el.parentElement
    ) {
      const cs = getComputedStyle(el)
      const scrollableX =
        cs.overflowX === 'auto' ||
        cs.overflowX === 'scroll' ||
        el.hasAttribute('data-overlayscrollbars-viewport')
      if (scrollableX && el.scrollWidth > el.clientWidth + 1) return
    }
    const t = e.touches[0]
    swipe.current = { x: t.clientX, y: t.clientY, active: false, dx: 0 }
  }
  const onTouchMove = (e: React.TouchEvent) => {
    const s = swipe.current
    if (!s) return
    const t = e.touches[0]
    const dx = t.clientX - s.x
    const dy = t.clientY - s.y
    if (!s.active) {
      if (Math.abs(dx) < 8 && Math.abs(dy) < 8) return
      // Vertical-dominant → let the body scroll; abandon the swipe.
      if (Math.abs(dy) > Math.abs(dx)) {
        swipe.current = null
        return
      }
      s.active = true
    }
    s.dx = dx
    const el = contentRef.current
    if (!el) return
    // Only follow in the close direction (don't over-drag inward).
    const translate = Math.max(0, dx * closeDir) * closeDir
    el.style.transition = 'none'
    el.style.transform = `translateX(${translate}px)`
  }
  const onTouchEnd = () => {
    const s = swipe.current
    swipe.current = null
    const el = contentRef.current
    if (!s || !s.active || !el) return
    el.style.transition = ''
    const width = el.getBoundingClientRect().width
    const moved = s.dx * closeDir
    el.style.transform = ''
    if (moved > Math.min(width * 0.35, 120)) {
      onClose?.()
    }
  }

  const maskClosable =
    maskClosableProp ?? (typeof mask === 'object' ? mask.closable !== false : mask !== false)
  const showOverlay = mask !== false

  // px size on the resize axis; full-bleed on the smallest breakpoint.
  const axisPx = width ?? (windowMinSize.xs && horizontal ? '100%' : sizePx(size))
  const sizeStyle: React.CSSProperties = horizontal ? { width: axisPx } : { height: axisPx }

  // A drawer must NOT dismiss (Escape / click-outside) while another drawer or
  // dialog is stacked ABOVE it — e.g. a file preview opened from inside this
  // drawer. Radix fires this lower layer's dismiss handlers too; guard them so
  // closing the top layer doesn't also close this one.
  const thisZ = zIndex ?? 50
  const higherLayerOpen = () => {
    const layers = document.querySelectorAll(
      '[data-testid="layout-drawer-content"], [data-slot="dialog-content"], [data-slot="alert-dialog-content"], [data-slot="sheet-content"]',
    )
    for (const el of layers) {
      if (el === contentRef.current) continue
      const z = parseInt(getComputedStyle(el).zIndex, 10)
      if (Number.isFinite(z) && z > thisZ) return true
    }
    return false
  }

  const footerNode = Array.isArray(footer) ? (
    // Array footers (e.g. [Cancel, Save]) right-align by convention.
    <div className="flex justify-end gap-2">
      {footer.map((item, i) => (
        <React.Fragment key={i}>{item}</React.Fragment>
      ))}
    </div>
  ) : (
    footer
  )

  const body = (
    // px-3 (not pr-3): the horizontal gutter must live INSIDE the scroll layer, or
    // the OverlayScrollbars viewport clips the left edge of an input's focus ring.
    // pb-4: breathing room so content scrolled to the bottom doesn't butt against
    // the footer band.
    <div className={cn('flex w-full h-full px-3 pb-4', classNames?.body)} style={styles?.body}>
      {React.Children.map(children, child =>
        React.isValidElement<{ className?: string }>(child)
          ? React.cloneElement(child, {
              ...child.props,
              className: `w-full ${child.props.className || ''}`.trim(),
            })
          : child,
      )}
    </div>
  )

  return (
    <DialogPrimitive.Root open={open} onOpenChange={o => { if (!o) onClose?.() }}>
      <DialogPrimitive.Portal>
        {showOverlay && (
          <DialogPrimitive.Overlay
            // Standard shadcn overlay (matches Dialog/Sheet): a faint tint + blur,
            // not a custom mask color.
            // z-40: keep the backdrop BELOW the drawer content (z-50). Radix
            // mounts the Overlay + Content as separate presence portals, and the
            // overlay can mount after the content — at equal z-index it would
            // then paint on top and swallow clicks on the drawer's own controls
            // (e.g. the Save button). A backdrop belongs under its content.
            // When a caller elevates the drawer via `zIndex` (a drawer opened
            // ON TOP of another drawer), the backdrop rides one below it so it
            // still covers the drawer underneath.
            style={zIndex != null ? { zIndex: zIndex - 1 } : undefined}
            // Swipe-to-close also works when the gesture starts on the mask (the
            // same handlers translate the panel via contentRef).
            onTouchStart={onTouchStart}
            onTouchMove={onTouchMove}
            onTouchEnd={onTouchEnd}
            className="fixed inset-0 z-40 bg-black/10 supports-backdrop-filter:backdrop-blur-xs data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0"
          />
        )}
        <DialogPrimitive.Content
          ref={contentRef}
          // Stable marker for "an app Drawer is open" — used by the page's
          // swipe-to-open-sidebar guard. NOT the data-testid (a caller can
          // override that, which would silently break the guard).
          data-slot="layout-drawer"
          data-testid={testid ?? 'layout-drawer-content'}
          // maskClosable=false → backdrop/outside click doesn't dismiss (Escape still does).
          // A higher-stacked drawer/dialog (e.g. a file preview opened from here)
          // being closed must not also dismiss THIS drawer.
          onEscapeKeyDown={e => { if (higherLayerOpen()) e.preventDefault() }}
          onPointerDownOutside={e => { if (!maskClosable || higherLayerOpen()) e.preventDefault() }}
          onInteractOutside={e => { if (!maskClosable || higherLayerOpen()) e.preventDefault() }}
          onTouchStart={onTouchStart}
          onTouchMove={onTouchMove}
          onTouchEnd={onTouchEnd}
          style={{ ...sizeStyle, zIndex }}
          className={cn(
            'fixed z-50 flex flex-col gap-0 bg-background shadow-none transition duration-200 ease-in-out',
            'data-[state=open]:animate-in data-[state=closed]:animate-out data-[state=open]:fade-in-0 data-[state=closed]:fade-out-0',
            sidePos[placement],
            // floating-card insets matching the LeftSidebar, full-bleed on xs.
            windowMinSize.xs
              ? 'border-0 rounded-none max-w-[100vw]'
              : 'ring-1 ring-foreground/10 rounded-lg m-2 ml-3 max-w-[calc(100vw-24px)]',
            className,
            classNames?.wrapper,
          )}
        >
          {title != null && (
            <div
              className={cn('flex w-full items-center gap-1 relative px-1 py-2 pt-[10px]', classNames?.header)}
              style={styles?.header}
            >
              {/* Back/dismiss affordance on the LEFT, before the title: the
                  glyph is an arrow-back (`‹`), which reads as "go back" and
                  belongs ahead of the title — a back arrow on the right edge is
                  confusing. (Supersedes the earlier J7 right-alignment: that used
                  the same back-arrow icon, where the right side read wrong.) */}
              {closable && (
                <Button variant="ghost" size="icon" tooltip="Close" aria-label="Close drawer" onClick={onClose} className="w-[30px]" data-testid="layout-drawer-close-button">
                  <span className="text-xl"><IoIosArrowBack aria-hidden="true" /></span>
                </Button>
              )}
              {typeof title === 'string' ? (
                // The visible heading IS the dialog's accessible name.
                <DialogPrimitive.Title asChild>
                  <Title level={5} className="!m-0">{title}</Title>
                </DialogPrimitive.Title>
              ) : (
                // A node title can't be introspected for text — render it
                // visually and label the dialog via an sr-only Title so Radix
                // still gets an accessible name (aria-labelledby).
                <>
                  <DialogPrimitive.Title className="sr-only">{titleText ?? 'Drawer'}</DialogPrimitive.Title>
                  {title}
                </>
              )}
              {/* Any caller-supplied header actions stay on the trailing edge. */}
              {extra != null && <div className="ms-auto flex items-center gap-1">{extra}</div>}
            </div>
          )}

          <div className="flex-1 min-h-0 pt-0">
            {noBodyScrollWrap ? body : <DivScrollY className="flex w-full h-full">{body}</DivScrollY>}
          </div>

          {footerNode != null && (
            <div
              // Standard footer band (matches Card/Dialog): a top separator + muted
              // fill, with the bottom corners rounded to the drawer card (square on
              // the xs full-bleed layout, which has no rounded corners).
              className={cn(
                'border-t bg-muted/50 px-4 py-3',
                windowMinSize.xs ? '' : 'rounded-b-lg',
                classNames?.footer,
              )}
              style={styles?.footer}
            >
              {footerNode}
            </div>
          )}

          {/* hidden a11y title when caller passes none (Radix requires a labelled dialog) */}
          {title == null && <DialogPrimitive.Title className="sr-only">Drawer</DialogPrimitive.Title>}

          {/* testid goes ON the handle (not a wrapper): the handle is
              position:absolute, so a wrapper collapses to a 0-height box and a
              drag targeting it would miss the real grab strip. With no wrapper,
              the handle's parent is the Content element → parentLevel 0. */}
          <ResizeHandle
            placement="left"
            parentLevel={[0]}
            testid="drawer-resize-handle"
          />
        </DialogPrimitive.Content>
      </DialogPrimitive.Portal>
    </DialogPrimitive.Root>
  )
}
