import * as React from 'react'

// Controlled/uncontrolled merged state: pass `value` to control, or omit it and the component owns
// state seeded from `defaultValue`. `onChange` always fires. Lets every kit control support
// defaultValue ↔ value without per-component boilerplate.
export function useControllableState<T>(opts: {
  value?: T
  defaultValue: T
  onChange?: (value: T) => void
}): [T, (next: T | ((prev: T) => T)) => void] {
  const { value, defaultValue, onChange } = opts
  const isControlled = value !== undefined
  const [internal, setInternal] = React.useState<T>(defaultValue)
  const current = isControlled ? (value as T) : internal
  // refs so `set` keeps a STABLE identity even when the caller passes an inline onChange / value.
  const currentRef = React.useRef(current)
  currentRef.current = current
  const onChangeRef = React.useRef(onChange)
  onChangeRef.current = onChange
  const isControlledRef = React.useRef(isControlled)
  isControlledRef.current = isControlled
  const set = React.useCallback((next: T | ((prev: T) => T)) => {
    const resolved = typeof next === 'function' ? (next as (p: T) => T)(currentRef.current) : next
    if (!isControlledRef.current) setInternal(resolved)
    onChangeRef.current?.(resolved)
  }, [])
  return [current, set]
}
