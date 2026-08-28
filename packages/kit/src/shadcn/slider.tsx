import { Slider as SliderPrimitive } from "@base-ui/react/slider"

import { cn } from "../lib/utils"

/**
 * The accessible NAME each thumb's nested `<input type="range">` answers to.
 *
 * A `Slider` is a `<div>` wrapping one `<input type="range">` PER THUMB, and the caller's
 * `aria-label` lands on the wrapper — so the inputs, which are the things a keyboard or screen
 * reader actually operates, had no name at all. axe reports that as a CRITICAL
 * `select-name`/`label` violation, and it is not a false one: the control a user reaches is
 * unnamed. It went unseen because no gallery surface rendered a slider until one did.
 *
 * A RANGE slider gets two distinguished names rather than one repeated one, for the same reason
 * a table column's N controls do: two operable controls sharing a name are indistinguishable to
 * anyone not looking at the screen. Base UI already supplies `aria-valuetext` ("3 start range"),
 * which says which VALUE — this says which CONTROL.
 *
 * With no `aria-label` on the root, no name is invented: a caller may be naming the control with
 * `aria-labelledby`, and a fabricated label would OVERRIDE it.
 */
function thumbLabel(label: unknown, index: number, count: number): string | undefined {
  if (typeof label !== "string" || label === "") return undefined
  if (count < 2) return label
  if (count === 2) return `${label} ${index === 0 ? "start" : "end"}`
  return `${label} ${index + 1}`
}

function Slider({
  className,
  defaultValue,
  value,
  min = 0,
  max = 100,
  ...props
}: SliderPrimitive.Root.Props) {
  const _values = Array.isArray(value)
    ? value
    : Array.isArray(defaultValue)
      ? defaultValue
      : [min, max]
  const rootLabel = (props as { "aria-label"?: unknown })["aria-label"]

  return (
    <SliderPrimitive.Root
      className={cn("data-horizontal:w-full data-vertical:h-full", className)}
      data-slot="slider"
      defaultValue={defaultValue}
      value={value}
      min={min}
      max={max}
      thumbAlignment="edge"
      {...props}
    >
      <SliderPrimitive.Control className="relative flex w-full touch-none items-center select-none data-disabled:opacity-50 data-vertical:h-full data-vertical:min-h-40 data-vertical:w-auto data-vertical:flex-col">
        <SliderPrimitive.Track
          data-slot="slider-track"
          className="relative grow overflow-hidden rounded-full bg-muted select-none data-horizontal:h-1 data-horizontal:w-full data-vertical:h-full data-vertical:w-1"
        >
          <SliderPrimitive.Indicator
            data-slot="slider-range"
            className="bg-primary select-none data-horizontal:h-full data-vertical:w-full"
          />
        </SliderPrimitive.Track>
        {Array.from({ length: _values.length }, (_, index) => (
          <SliderPrimitive.Thumb
            data-slot="slider-thumb"
            key={index}
            // `index` is what lets Base UI bind the RIGHT value to this thumb's input without a
            // client render; `getAriaLabel` is what names that input. Both are per-thumb, so
            // neither can be supplied on the root.
            index={index}
            getAriaLabel={
              thumbLabel(rootLabel, index, _values.length) === undefined
                ? undefined
                : (i: number) => thumbLabel(rootLabel, i, _values.length)!
            }
            className="relative block size-3 shrink-0 rounded-full border border-ring bg-white ring-ring/50 transition-[color,box-shadow] select-none after:absolute after:-inset-2 hover:ring-3 focus-visible:ring-3 focus-visible:outline-hidden active:ring-3 disabled:pointer-events-none disabled:opacity-50"
          />
        ))}
      </SliderPrimitive.Control>
    </SliderPrimitive.Root>
  )
}

export { Slider }
