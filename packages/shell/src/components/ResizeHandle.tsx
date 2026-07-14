import { CSSProperties, useLayoutEffect, useRef, useState } from 'react'

export const ResizeHandle = ({
  placement,
  parentLevel,
  style,
  onStart,
  onEnd,
  scale,
  className,
  testid,
  minWidth = 300,
  minHeight = 200,
  maxWidth = Infinity,
  maxHeight = Infinity,
}: {
  placement: 'top' | 'right' | 'bottom' | 'left'
  parentLevel?: number | number[] //to determine the parent element
  style?: CSSProperties
  onStart?: () => void
  onEnd?: () => void
  scale?: number
  className?: string
  /**
   * Applied to the actual interactive (absolutely-positioned, full-edge) handle
   * element — NOT a wrapper. A wrapper around this component collapses to a
   * zero-size box (the handle is `position: absolute`), so a test targeting the
   * wrapper would grab geometry that doesn't match the real grab strip. Pass
   * the testid here so it lands on the element a user actually drags.
   */
  testid?: string
  minWidth?: number
  minHeight?: number
  maxWidth?: number
  maxHeight?: number
}) => {
  const ref = useRef<HTMLDivElement>(null)
  const parentRefs = useRef<HTMLElement[]>([])
  // Current resized dimension (px) of the primary parent — surfaced as
  // `aria-valuenow` so this focusable role="separator" (a window-splitter)
  // carries the ARIA attributes the pattern requires (axe: aria-required-attr).
  const [valueNow, setValueNow] = useState(0)
  // Teardown for an in-flight drag (set on mousedown, cleared on mouseup). Lets
  // the unmount effect below detach the window mousemove/mouseup listeners if
  // the handle unmounts mid-drag — otherwise those closures would leak.
  const dragCleanupRef = useRef<(() => void) | null>(null)

  const parentLevels = Array.isArray(parentLevel)
    ? parentLevel
    : [parentLevel || 0]

  scale = scale ?? 1

  // Resolve the parent element(s) the handle resizes. `useLayoutEffect` runs
  // synchronously after the DOM is committed (including portal content, which
  // mounts in the same commit), so `parentElement` is already available — no
  // arbitrary setTimeout needed. parentRefs is only read on user interaction
  // (the keyboard/drag handlers below), well after this resolves.
  useLayoutEffect(() => {
    const refs: HTMLElement[] = []
    for (const parentLevel of parentLevels) {
      let parent = ref.current?.parentElement ?? null
      let level = 0
      while (parent && level < parentLevel) {
        parent = parent.parentElement
        level++
      }
      if (!parent) continue
      refs.push(parent)
    }
    parentRefs.current = refs
    const primary = refs[0]
    if (primary) {
      setValueNow(
        placement === 'left' || placement === 'right'
          ? primary.offsetWidth
          : primary.offsetHeight
      )
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const classNames = [
    'absolute',
    'transition-opacity',
    'duration-200',
    'pointer-events-auto',
    className,
  ]
  if (['top', 'bottom'].includes(placement)) {
    classNames.push('w-full', 'h-1', 'cursor-row-resize')
  } else {
    classNames.push('h-full', 'w-1', 'cursor-col-resize')
  }

  if (placement === 'top') classNames.push('top-0', 'left-0')
  if (placement === 'right') classNames.push('top-0', 'right-0')
  if (placement === 'bottom') classNames.push('bottom-0', 'left-0')
  if (placement === 'left') classNames.push('top-0', 'left-0')

  const isHorizontal = placement === 'left' || placement === 'right'

  return (
    <div className={classNames.join(' ')} ref={ref} style={style} data-testid={testid}>
      <div
        className={'w-full h-full relative group'}
        role="separator"
        aria-orientation={isHorizontal ? 'vertical' : 'horizontal'}
        aria-label="Resize"
        aria-valuenow={Math.round(valueNow)}
        aria-valuemin={isHorizontal ? minWidth : minHeight}
        {...(Number.isFinite(isHorizontal ? maxWidth : maxHeight)
          ? { 'aria-valuemax': isHorizontal ? maxWidth : maxHeight }
          : {})}
        tabIndex={0}
        onKeyDown={event => {
          // Keyboard actuation: nudge the parent dimension in the same
          // direction the drag would, clamped to the same min/max. Shift =
          // larger step. Mirrors the mouse path so size persists identically.
          const step = event.shiftKey ? 48 : 16
          const targets = parentRefs.current
          if (!targets.length) return
          let grow = 0 // +1 grow, -1 shrink, 0 = key not handled
          if (isHorizontal) {
            if (event.key === 'ArrowRight') grow = placement === 'left' ? -1 : 1
            else if (event.key === 'ArrowLeft') grow = placement === 'left' ? 1 : -1
          } else {
            if (event.key === 'ArrowDown') grow = placement === 'top' ? -1 : 1
            else if (event.key === 'ArrowUp') grow = placement === 'top' ? 1 : -1
          }
          if (grow === 0) return
          event.preventDefault()
          if (onStart) onStart()
          for (const target of targets) {
            if (isHorizontal) {
              let w = target.offsetWidth + grow * step
              w = Math.min(Math.max(w, minWidth), maxWidth)
              target.style.width = `${w}px`
              setValueNow(w)
            } else {
              let h = target.offsetHeight + grow * step
              h = Math.min(Math.max(h, minHeight), maxHeight)
              target.style.height = `${h}px`
              setValueNow(h)
            }
          }
          if (onEnd) onEnd()
        }}
        onMouseDown={event => {
          event.preventDefault()
          event.stopPropagation()

          if (onStart) onStart()

          const targets = parentRefs.current
          if (!targets.length) return

          // Per-target listener removers, aggregated so the unmount effect can
          // detach ALL of them if the handle disappears mid-drag.
          const dragCleanups: Array<() => void> = []

          for (const target of targets) {
            //disable css transition to prevent flickering
            const currentTransition = target.style.transition
            target.style.transition = 'none'

            const targetWindow = target.ownerDocument.defaultView!

            const currentPos = {
              top: event.clientY,
              left: event.clientX,
            }

            const currentDim = {
              width: target.offsetWidth,
              height: target.offsetHeight,
            }

            const previousDim = {
              width: target.offsetWidth,
              height: target.offsetHeight,
            }

            const currentScreenPos = {
              top: ref.current!.getBoundingClientRect().top,
              left: ref.current!.getBoundingClientRect().left,
            }

            const moveHandler = (e: MouseEvent) => {
              e.preventDefault()
              e.stopPropagation()

              const newPos = {
                top: e.clientY,
                left: e.clientX,
              }

              const newDim = {
                width:
                  placement === 'left'
                    ? currentDim.width +
                      (currentPos.left - newPos.left) * scale!
                    : currentDim.width +
                      (newPos.left - currentPos.left) * scale!,
                height:
                  placement === 'top'
                    ? currentDim.height + (currentPos.top - newPos.top) * scale!
                    : currentDim.height +
                      (newPos.top - currentPos.top) * scale!,
              }

              newDim.width = Math.max(newDim.width, minWidth)
              newDim.height = Math.max(newDim.height, minHeight)
              newDim.width = Math.min(newDim.width, maxWidth)
              newDim.height = Math.min(newDim.height, maxHeight)

              if (['top', 'bottom'].includes(placement)) {
                target.style.height = `${newDim.height}px`
                setValueNow(newDim.height)
              } else {
                target.style.width = `${newDim.width}px`
                setValueNow(newDim.width)
              }

              const newScreenPos = {
                top: ref.current!.getBoundingClientRect().top,
                left: ref.current!.getBoundingClientRect().left,
              }

              //restore old width/height if the element is at peak width/height
              if (
                currentScreenPos.top === newScreenPos.top &&
                currentScreenPos.left === newScreenPos.left
              ) {
                if (['top', 'bottom'].includes(placement)) {
                  target.style.height = `${previousDim.height}px`
                } else {
                  target.style.width = `${previousDim.width}px`
                }

                return
              }

              previousDim.width = newDim.width
              previousDim.height = newDim.height

              currentScreenPos.top = newScreenPos.top
              currentScreenPos.left = newScreenPos.left
            }

            const upHandler = (e: MouseEvent) => {
              e.preventDefault()
              e.stopPropagation()
              targetWindow.removeEventListener('mousemove', moveHandler)
              targetWindow.removeEventListener('mouseup', upHandler)
              target.style.transition = currentTransition
              dragCleanupRef.current = null

              if (onEnd) onEnd()
            }

            targetWindow.addEventListener('mousemove', moveHandler)
            targetWindow.addEventListener('mouseup', upHandler)
            dragCleanups.push(() => {
              targetWindow.removeEventListener('mousemove', moveHandler)
              targetWindow.removeEventListener('mouseup', upHandler)
              target.style.transition = currentTransition
            })
          }

          // Expose an aggregate teardown for the unmount effect; the (shared)
          // mouseup clears it once the drag completes normally.
          dragCleanupRef.current = () => dragCleanups.forEach(c => c())
        }}
      >
        {isHorizontal && (
          <div className="absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 w-[3px] h-10 rounded-full bg-muted-foreground opacity-0 group-hover:opacity-60 transition-opacity pointer-events-none" />
        )}
      </div>
    </div>
  )
}
