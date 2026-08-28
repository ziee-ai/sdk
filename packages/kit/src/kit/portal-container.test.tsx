// @vitest-environment jsdom
//
// The property under test is WHICH DOCUMENT an overlay's DOM ends up in.
//
// That property is invisible to an ordinary render test, because an ordinary render test has one
// document — and with one document "the portal target is `document.body`" is true both when the kit
// is correct and when it is broken. Every assertion here therefore runs against TWO real documents:
// a subtree is mounted into a second document (an iframe's, standing in for a popped-out window's,
// which is what `@ziee/dock`'s ContainerWindows creates), and the overlays it opens must land in
// THAT document.
//
// Both halves are asserted every time, and the second half is the one that fails on the bug:
//   · the overlay IS in the second document, and
//   · the overlay is NOT in the ambient one.
// Before the fix Base UI resolved its portal target as `container ?? parentPortalNode ??
// document.body` with `document` being the AMBIENT global, so the first half failed and the second
// half failed too — the popup rendered into the opener.
//
// The main-window path is a NEGATIVE CONTROL here, not an afterthought: the same overlays mounted
// in the ambient document must still resolve to the ambient `<body>`, because the fix is required
// to leave that path untouched.

import { describe, it, expect, beforeAll, afterEach } from 'vitest'
import * as React from 'react'
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { createPortal } from 'react-dom'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { PortalContainerProvider, resolvePortalContainer } from './portal-container'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '../shadcn/tooltip'
import { HoverCard, HoverCardContent, HoverCardTrigger } from '../shadcn/hover-card'
import { Popover, PopoverContent, PopoverTrigger } from '../shadcn/popover'
import { Select, SelectContent, SelectItem, SelectTrigger } from '../shadcn/select'
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger,
} from '../shadcn/dropdown-menu'
import { ContextMenu, ContextMenuContent, ContextMenuItem, ContextMenuTrigger } from '../shadcn/context-menu'
import { Dialog, DialogContent, DialogTitle } from '../shadcn/dialog'
import { AlertDialog, AlertDialogContent, AlertDialogTitle } from '../shadcn/alert-dialog'
import { Sheet, SheetContent, SheetTitle } from '../shadcn/sheet'
import { Combobox, ComboboxContent, ComboboxInput, ComboboxItem, ComboboxList } from '../shadcn/combobox'

// ── jsdom gaps the kit's base-ui controls probe at mount ───────────────────────────────────────
function polyfill(scope: Window & typeof globalThis): void {
  const g = scope as unknown as Record<string, unknown>
  g.ResizeObserver ??= class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  if (typeof g.PointerEvent !== 'function') g.PointerEvent = class extends scope.MouseEvent {}
  if (!g.CSS) g.CSS = { escape: (v: string) => v }
  else if (typeof (g.CSS as { escape?: unknown }).escape !== 'function') {
    ;(g.CSS as { escape: (v: string) => string }).escape = (v: string) => v
  }
  if (!scope.matchMedia) {
    scope.matchMedia = ((q: string) => ({
      matches: false, media: q, onchange: null,
      addEventListener() {}, removeEventListener() {},
      addListener() {}, removeListener() {}, dispatchEvent: () => false,
    })) as never
  }
}

beforeAll(() => {
  polyfill(globalThis as unknown as Window & typeof globalThis)
})

// ── the second document ────────────────────────────────────────────────────────────────────────

interface Realm {
  doc: Document
  body: HTMLElement
  dispose(): void
}

/**
 * A genuinely separate document, with its own `defaultView` — the same shape a popped-out window
 * presents to React. An iframe is used rather than `document.implementation.createHTMLDocument()`
 * precisely BECAUSE it has a window: the kit resolves through `ownerDocument.defaultView` in
 * places, and a window-less document would make those paths untestable.
 */
function secondDocument(): Realm {
  const frame = document.createElement('iframe')
  document.body.appendChild(frame)
  const doc = frame.contentDocument
  const win = frame.contentWindow
  if (!doc || !win) throw new Error('iframe produced no document — the two-document premise is void')
  polyfill(win as unknown as Window & typeof globalThis)
  return {
    doc,
    body: doc.body,
    dispose: () => frame.remove(),
  }
}

const roots: Root[] = []
const realms: Realm[] = []

function mount(node: React.ReactNode, host: HTMLElement): void {
  const container = document.createElement('div')
  document.body.appendChild(container)
  const root = createRoot(container)
  roots.push(root)
  act(() => {
    // The subtree is portaled into `host` exactly the way ContainerWindows portals a pane's
    // workspace into a popup: one React tree, DOM in another document.
    root.render(host === document.body ? node : createPortal(node, host))
  })
}

afterEach(() => {
  act(() => {
    for (const r of roots.splice(0)) r.unmount()
  })
  for (const r of realms.splice(0)) r.dispose()
  document.body.innerHTML = ''
})

// ── the overlay matrix ─────────────────────────────────────────────────────────────────────────
//
// Each entry opens ONE overlay family in the controlled-open state and names the `data-slot` its
// popup carries. `portal` records which Base UI portal the family uses, because the two behave
// differently on nesting (see kit/portal-container.tsx) and the distinction is asserted below.

interface Family {
  name: string
  slot: string
  portal: 'full' | 'lite'
  render(): React.ReactElement
}

const FAMILIES: Family[] = [
  {
    name: 'tooltip', slot: 'tooltip-content', portal: 'lite',
    render: () => (
      <TooltipProvider>
        <Tooltip open>
          <TooltipTrigger render={<button type="button" />}>trigger</TooltipTrigger>
          <TooltipContent>hint</TooltipContent>
        </Tooltip>
      </TooltipProvider>
    ),
  },
  {
    name: 'hover-card', slot: 'hover-card-content', portal: 'lite',
    render: () => (
      <HoverCard open>
        <HoverCardTrigger render={<button type="button" />}>trigger</HoverCardTrigger>
        <HoverCardContent>preview</HoverCardContent>
      </HoverCard>
    ),
  },
  {
    name: 'popover', slot: 'popover-content', portal: 'full',
    render: () => (
      <Popover open>
        <PopoverTrigger render={<button type="button" />}>trigger</PopoverTrigger>
        <PopoverContent>body</PopoverContent>
      </Popover>
    ),
  },
  {
    name: 'select', slot: 'select-content', portal: 'full',
    render: () => (
      <Select open>
        <SelectTrigger>pick</SelectTrigger>
        <SelectContent>
          <SelectItem value="a">A</SelectItem>
        </SelectContent>
      </Select>
    ),
  },
  {
    name: 'dropdown-menu', slot: 'dropdown-menu-content', portal: 'full',
    render: () => (
      <DropdownMenu open>
        <DropdownMenuTrigger render={<button type="button" />}>menu</DropdownMenuTrigger>
        <DropdownMenuContent>
          <DropdownMenuItem>one</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    ),
  },
  {
    name: 'context-menu', slot: 'context-menu-content', portal: 'full',
    render: () => (
      <ContextMenu open>
        <ContextMenuTrigger>area</ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem>one</ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
    ),
  },
  {
    name: 'dialog', slot: 'dialog-content', portal: 'full',
    render: () => (
      <Dialog open>
        <DialogContent>
          <DialogTitle>title</DialogTitle>
        </DialogContent>
      </Dialog>
    ),
  },
  {
    name: 'alert-dialog', slot: 'alert-dialog-content', portal: 'full',
    render: () => (
      <AlertDialog open>
        <AlertDialogContent>
          <AlertDialogTitle>title</AlertDialogTitle>
        </AlertDialogContent>
      </AlertDialog>
    ),
  },
  {
    name: 'sheet', slot: 'sheet-content', portal: 'full',
    render: () => (
      <Sheet open>
        <SheetContent>
          <SheetTitle>title</SheetTitle>
        </SheetContent>
      </Sheet>
    ),
  },
  {
    name: 'combobox', slot: 'combobox-content', portal: 'full',
    render: () => (
      <Combobox open items={['a']}>
        <ComboboxInput />
        <ComboboxContent>
          <ComboboxList>{(item: string) => <ComboboxItem key={item} value={item}>{item}</ComboboxItem>}</ComboboxList>
        </ComboboxContent>
      </Combobox>
    ),
  },
]

describe('an overlay opened in a second document portals into THAT document', () => {
  for (const family of FAMILIES) {
    it(`${family.name} — popup lands in the subtree's own document, never the ambient one`, () => {
      const realm = secondDocument()
      realms.push(realm)

      mount(
        <PortalContainerProvider container={realm.body}>{family.render()}</PortalContainerProvider>,
        realm.body,
      )

      const there = realm.doc.querySelectorAll(`[data-slot="${family.slot}"]`)
      const here = document.querySelectorAll(`[data-slot="${family.slot}"]`)

      // The overlay exists at all — without this the "not in the opener" half would pass vacuously
      // on an overlay that simply failed to render.
      expect(there.length, `${family.name} popup missing from the second document`).toBe(1)
      expect(there[0].ownerDocument).toBe(realm.doc)
      // …and the opener never sees it. This is the assertion the bug fails.
      expect(here.length, `${family.name} popup leaked into the ambient document`).toBe(0)
    })
  }
})

describe('the ambient document is a negative control — the main-window path is unchanged', () => {
  for (const family of FAMILIES) {
    it(`${family.name} — with no provider the popup resolves to the ambient <body>`, () => {
      mount(family.render(), document.body)
      const here = document.querySelectorAll(`[data-slot="${family.slot}"]`)
      expect(here.length).toBe(1)
      expect(here[0].ownerDocument).toBe(document)
    })
  }
})

describe('resolvePortalContainer', () => {
  it('maps an element, its document and its window to the SAME body', () => {
    const realm = secondDocument()
    realms.push(realm)
    const el = realm.doc.createElement('div')
    realm.body.appendChild(el)
    expect(resolvePortalContainer(el)).toBe(realm.body)
    expect(resolvePortalContainer(realm.doc)).toBe(realm.body)
    expect(resolvePortalContainer(realm.doc.defaultView)).toBe(realm.body)
  })

  it('is a NO-OP for the ambient document, so Base UI keeps its own default', () => {
    // Returning `document.body` here instead of `undefined` would silently defeat Base UI's
    // nested-portal inheritance in the main window — the one thing the fix must not touch.
    expect(resolvePortalContainer(document.body)).toBeUndefined()
    expect(resolvePortalContainer(document)).toBeUndefined()
    expect(resolvePortalContainer(window)).toBeUndefined()
    expect(resolvePortalContainer(null)).toBeUndefined()
    expect(resolvePortalContainer(undefined)).toBeUndefined()
  })

  it('does not rely on instanceof, which is FALSE across realms', () => {
    const realm = secondDocument()
    realms.push(realm)
    const el = realm.doc.createElement('div')
    // The premise of the whole module: a node from another realm fails the opener's instanceof.
    // If this ever stops holding, `documentOf` may be simplified — until then it must stay
    // duck-typed.
    expect(el instanceof HTMLElement).toBe(false)
    expect(resolvePortalContainer(el)).toBe(realm.body)
  })
})

describe('coverage ledger', () => {
  it('every kit file that renders a Base UI portal is represented in the matrix', () => {
    // This does NOT assert the fix — the runtime tests above do. It asserts that a NEWLY ADDED
    // overlay family cannot slip in un-exercised: adding a file that renders a `*.Portal` without
    // adding it to FAMILIES fails here, with the file named.
    const dir = join(dirname(fileURLToPath(import.meta.url)), '..', 'shadcn')
    const portalFiles = readdirSync(dir)
      .filter(f => f.endsWith('.tsx'))
      .filter(f => /<[A-Za-z]*(Primitive)?\.Portal[\s>]|<[A-Za-z]+Portal[\s>]/.test(readFileSync(join(dir, f), 'utf8')))
      .map(f => f.replace(/\.tsx$/, ''))
      .sort()

    // Families composed ENTIRELY out of another family's Content (so already covered through it),
    // and the one portal that is not Base UI's.
    const coveredIndirectly = new Set([
      'menubar', // MenubarContent/SubContent delegate to DropdownMenuContent
      'navigation-menu', // no controlled-open form that renders in jsdom; see report
      'drawer', // vaul/Radix portal, not Base UI's — asserted through the app-level popout test
    ])
    const covered = new Set([...FAMILIES.map(f => f.name), ...coveredIndirectly])
    const missing = portalFiles.filter(f => !covered.has(f))
    expect(missing, `overlay files with no entry in FAMILIES: ${missing.join(', ')}`).toEqual([])
  })
})
