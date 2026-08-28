"use client"

import * as React from "react"
import { Dialog as DialogPrimitive } from "@base-ui/react/dialog"

import { cn } from "../lib/utils"
import { OverlayCloseButton } from "../internal/overlay-close"
import { Button } from "./button"
import { usePortalContainer, PortalContainerInherit } from "../kit/portal-container"

function Dialog({ ...props }: DialogPrimitive.Root.Props) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({ ...props }: DialogPrimitive.Trigger.Props) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: DialogPrimitive.Portal.Props) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

/**
 * The nearest enclosing focus-trapping modal layer that base-ui's Dialog does
 * NOT participate in — a Radix Dialog (the app-layout `Drawer`) or a vaul Drawer
 * (also Radix-based). When a base-ui Dialog is opened from inside such a layer,
 * the layer's focus scope yanks focus back out of our `<body>`-portaled popup, so
 * its inputs can't be typed into and React `onChange` never fires. Portaling the
 * popup INTO that layer's subtree keeps it inside the focus scope.
 *
 * Matches Radix Dialog/AlertDialog content (`role` + `data-state`, which vaul
 * inherits). Base-ui popups (Sheet, nested kit Dialogs) use `data-open`/
 * `data-closed` — never `data-state` — so they don't match and keep base-ui's
 * native, already-working nesting via the default `<body>` portal.
 */
const HOST_FOCUS_TRAP_SELECTOR =
  '[role="dialog"][data-state], [role="alertdialog"][data-state], [data-slot="drawer-content"]'

function DialogClose({ ...props }: DialogPrimitive.Close.Props) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({
  className,
  ...props
}: DialogPrimitive.Backdrop.Props) {
  return (
    <DialogPrimitive.Backdrop
      data-slot="dialog-overlay"
      // forceRender: Base-UI skips a nested dialog's backdrop by default
      // (enabled = forceRender || !nested). A Dialog opened from inside the
      // mobile sidebar Sheet is "nested", so without this it renders no backdrop
      // and the sidebar isn't dimmed/blurred behind it.
      forceRender
      className={cn(
        "fixed inset-0 isolate z-[55] bg-black/10 duration-100 supports-backdrop-filter:backdrop-blur-xs data-open:animate-in data-open:fade-in-0 data-closed:animate-out data-closed:fade-out-0",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  closeLabel = "Close",
  container,
  ...props
}: DialogPrimitive.Popup.Props & {
  showCloseButton?: boolean
  /** Accessible name AND hover text for the corner ×. Defaults to the English word this
      component has always hardcoded in its `sr-only` span, so supplying it is optional and
      omitting it is not a regression — but a translated app should supply it. */
  closeLabel?: string
  /** Portal target for the popup. Defaults to the `<body>` of the document this
      subtree renders in (see kit/portal-container.tsx); the kit Dialog sets this
      to a host focus-trap (e.g. an enclosing vaul Drawer) so inputs stay typable
      — see kit/dialog.tsx. An explicit value always wins. */
  container?: DialogPrimitive.Portal.Props["container"]
}) {
  const portalContainer = usePortalContainer(container)
  return (
    <DialogPortal container={portalContainer}>
     <PortalContainerInherit>
      <DialogOverlay />
      <DialogPrimitive.Popup
        data-slot="dialog-content"
        className={cn(
          // z-[60] + pointer-events-auto so a Dialog opened INSIDE a modal
          // Drawer (Radix Dialog, which sets body{pointer-events:none} and
          // renders at z-50) is above it and its buttons stay clickable — the
          // Base-UI dialog is a separate body portal that would otherwise
          // inherit pointer-events:none and have clicks fall through.
          "fixed top-1/2 left-1/2 z-[60] pointer-events-auto grid w-full max-w-[calc(100%-2rem)] -translate-x-1/2 -translate-y-1/2 gap-4 rounded-xl bg-popover p-4 text-sm text-popover-foreground ring-1 ring-foreground/10 duration-100 outline-none sm:max-w-sm data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <OverlayCloseButton slot="dialog-close" label={closeLabel} className="absolute top-2 right-2" />
        )}
      </DialogPrimitive.Popup>
     </PortalContainerInherit>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2", className)}
      {...props}
    />
  )
}

function DialogFooter({
  className,
  showCloseButton = false,
  children,
  ...props
}: React.ComponentProps<"div"> & {
  showCloseButton?: boolean
}) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn(
        "-mx-4 -mb-4 flex flex-col-reverse gap-2 rounded-b-xl border-t bg-muted/50 p-4 sm:flex-row sm:justify-end",
        className
      )}
      {...props}
    >
      {children}
      {showCloseButton && (
        <DialogPrimitive.Close render={<Button variant="outline" />}>
          Close
        </DialogPrimitive.Close>
      )}
    </div>
  )
}

function DialogTitle({ className, ...props }: DialogPrimitive.Title.Props) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn(
        "text-base leading-none font-medium",
        className
      )}
      {...props}
    />
  )
}

function DialogDescription({
  className,
  ...props
}: DialogPrimitive.Description.Props) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn(
        "text-sm text-muted-foreground *:[a]:underline *:[a]:underline-offset-3 *:[a]:hover:text-foreground",
        className
      )}
      {...props}
    />
  )
}

export {
  HOST_FOCUS_TRAP_SELECTOR,
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
