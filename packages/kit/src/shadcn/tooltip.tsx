import { Tooltip as TooltipPrimitive } from "@base-ui/react/tooltip"

import { cn } from "../lib/utils"
import { usePortalContainer, type PortalContainerValue } from "../kit/portal-container"

function TooltipProvider({
  delay = 0,
  ...props
}: TooltipPrimitive.Provider.Props) {
  return (
    <TooltipPrimitive.Provider
      data-slot="tooltip-provider"
      delay={delay}
      {...props}
    />
  )
}

function Tooltip({ ...props }: TooltipPrimitive.Root.Props) {
  return <TooltipPrimitive.Root data-slot="tooltip" {...props} />
}

function TooltipTrigger({ ...props }: TooltipPrimitive.Trigger.Props) {
  return <TooltipPrimitive.Trigger data-slot="tooltip-trigger" {...props} />
}

// The arrow is a SQUARE rotated 45°, so which of its four edges face OUTWARD depends on where the
// popup sits — and only the outward pair may be drawn. CSS `rotate(45deg)` is clockwise in screen
// coordinates (y points down), which sends each edge's outward normal to:
//
//     border-t → up-right     border-r → down-right
//     border-l → up-left      border-b → down-left
//
// The arrow pokes TOWARD the trigger, and `data-side` names where the POPUP is relative to it, so
// the outward pair is the two edges whose normals point the way the arrow pokes:
//
//     data-side=top    (popup above,  pokes down)   → border-b border-r
//     data-side=bottom (popup below,  pokes up)     → border-t border-l
//     data-side=left   (popup left,   pokes right)  → border-t border-r
//     data-side=right  (popup right,  pokes left)   → border-b border-l
//
// Before this was per-side, `border-r border-b` was hardcoded — the `top` row above — so every
// other placement drew its edge on the two INWARD faces. Reported as "the arrow always has a
// bottom shadow and looks like it is only for top tooltip", which is exactly what that is.
//
// LOGICAL SIDES. Base UI emits `data-side="inline-start"/"inline-end"` (rather than a physical
// side) whenever the CONSUMER passed a logical `side`, and those resolve by writing direction —
// `getLogicalSide` in @base-ui/react maps inline-end to physical RIGHT under ltr and LEFT under
// rtl. The rotation itself is physical and does NOT flip, and the correct pair differs in its
// BLOCK component too (bottom+left vs top+right), which no logical border property can express —
// the block axis does not mirror with `dir`. So the two logical sides carry explicit `ltr:`/`rtl:`
// arms. Their INSET is logical (`-start-1`/`-end-1`, identical to the old `-left-1`/`-right-1`
// under ltr), which also repairs their placement under rtl.
//
// `rounded-[2px]` is deliberately left uniform: it applies to all four corners, so unlike the
// borders it is already side-invariant and needs no per-side arm.
const tooltipArrowClasses =
  "z-50 size-2.5 translate-y-[calc(-50%-2px)] rotate-45 rounded-[2px] border-border bg-popover fill-popover " +
  // placement
  "data-[side=top]:-bottom-2.5 " +
  "data-[side=bottom]:top-1 " +
  "data-[side=left]:top-1/2! data-[side=left]:-right-1 data-[side=left]:-translate-y-1/2 " +
  "data-[side=right]:top-1/2! data-[side=right]:-left-1 data-[side=right]:-translate-y-1/2 " +
  "data-[side=inline-start]:top-1/2! data-[side=inline-start]:-end-1 data-[side=inline-start]:-translate-y-1/2 " +
  "data-[side=inline-end]:top-1/2! data-[side=inline-end]:-start-1 data-[side=inline-end]:-translate-y-1/2 " +
  // the outward edge pair, per resolved side
  "data-[side=top]:border-r data-[side=top]:border-b " +
  "data-[side=bottom]:border-t data-[side=bottom]:border-l " +
  "data-[side=left]:border-t data-[side=left]:border-r " +
  "data-[side=right]:border-b data-[side=right]:border-l " +
  "ltr:data-[side=inline-start]:border-t ltr:data-[side=inline-start]:border-r " +
  "rtl:data-[side=inline-start]:border-b rtl:data-[side=inline-start]:border-l " +
  "ltr:data-[side=inline-end]:border-b ltr:data-[side=inline-end]:border-l " +
  "rtl:data-[side=inline-end]:border-t rtl:data-[side=inline-end]:border-r"

function TooltipContent({
  className,
  side = "top",
  sideOffset = 4,
  align = "center",
  alignOffset = 0,
  children,
  container,
  ...props
}: TooltipPrimitive.Popup.Props &
  Pick<
    TooltipPrimitive.Positioner.Props,
    "align" | "alignOffset" | "side" | "sideOffset"
  > & {
    /** Portal target. Defaults to the document of the window this subtree renders in. */
    container?: PortalContainerValue
  }) {
  // Tooltip uses Base UI's LITE portal, which publishes no portal node to descendants — so the
  // container must keep flowing down (no PortalContainerInherit here).
  const portalContainer = usePortalContainer(container)
  return (
    <TooltipPrimitive.Portal container={portalContainer}>
      <TooltipPrimitive.Positioner
        align={align}
        alignOffset={alignOffset}
        side={side}
        sideOffset={sideOffset}
        // z-[2000]: a tooltip is a transient hint that must sit above every
        // surface — including elevated drawers (the file preview uses z-1050).
        className="isolate z-[2000]"
      >
        <TooltipPrimitive.Popup
          data-slot="tooltip-content"
          className={cn(
            "z-50 inline-flex w-fit max-w-xs origin-(--transform-origin) items-center gap-1.5 rounded-md border border-border bg-popover px-3 py-1.5 text-xs text-popover-foreground shadow-md has-data-[slot=kbd]:pr-1.5 data-[side=bottom]:slide-in-from-top-2 data-[side=inline-end]:slide-in-from-left-2 data-[side=inline-start]:slide-in-from-right-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 **:data-[slot=kbd]:relative **:data-[slot=kbd]:isolate **:data-[slot=kbd]:z-50 **:data-[slot=kbd]:rounded-sm data-[state=delayed-open]:animate-in data-[state=delayed-open]:fade-in-0 data-[state=delayed-open]:zoom-in-95 data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95",
            className
          )}
          {...props}
        >
          {children}
          <TooltipPrimitive.Arrow className={tooltipArrowClasses} />
        </TooltipPrimitive.Popup>
      </TooltipPrimitive.Positioner>
    </TooltipPrimitive.Portal>
  )
}

export { Tooltip, TooltipTrigger, TooltipContent, TooltipProvider }
