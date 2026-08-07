// portal-container — the ONE place the kit decides WHICH DOCUMENT an overlay portals into.
//
// WHY THIS EXISTS
// A React subtree can be `createPortal`ed into a SEPARATE window's document (the dock's
// `ContainerWindows` does exactly that for a popped-out pane; viz's `useWindowPortal` does it for a
// popped-out plot). React keeps rendering it as part of ONE tree, but its DOM lives in another
// realm. Every kit overlay is portaled by Base UI, and Base UI's portal resolves its target as
//
//     containerProp ?? parentPortalNode ?? document.body      (floating-ui-react/FloatingPortal)
//
// where `document` is the AMBIENT global — i.e. the OPENER's document. That single line is the only
// global-`document` reference in the whole of `@base-ui/react`'s production code (everything else
// resolves through `ownerDocument`), which is why threading a container is sufficient to make the
// entire overlay family cross-window-correct: positioning, outside-press dismissal, Escape and
// focus restoration all already resolve from the node's own document.
//
// THE CONTRACT
//   • `PortalContainerProvider` is installed ONCE at each cross-document boundary. It takes any
//     handle to the far side (an Element, a Document or a Window) and derives that document's
//     `<body>` itself — no caller ever types a document, and a boundary cannot supply the wrong
//     kind of handle.
//   • It resolves to `undefined` whenever the derived document IS the ambient one. So the provider
//     is a NO-OP in the main window: overlays keep passing `container={undefined}` exactly as they
//     did before this module existed, and Base UI's own default (including its nested-portal
//     `parentPortalNode` inheritance) is untouched. The main-window path does not change.
//   • Every kit overlay reads `usePortalContainer()`. Nothing opts in per call site, so a newly
//     added overlay is cross-window-correct by construction and a newly added overlay CALL SITE
//     cannot forget anything.
//   • An explicit `container` prop always wins and is passed through VERBATIM (never re-derived) —
//     `kit/dialog.tsx` uses it to portal into an enclosing focus trap in the SAME document, which
//     deriving would destroy.
//
// NESTING
// Base UI's full portal publishes its own portal node to descendants, so a Select opened inside a
// Dialog portals INSIDE the Dialog's portal node rather than beside it — which is what keeps the
// Dialog's outside-press logic from treating a click on the Select as "outside". Overriding the
// container on the inner overlay too would flatten that into siblings. So an overlay whose portal
// propagates that node wraps its portal CHILDREN in `PortalContainerInherit`, handing descendants
// back to Base UI's inheritance. Tooltip and PreviewCard use the "lite" portal, which publishes
// nothing — for those the container must keep flowing down, or a descendant would fall back to the
// ambient `document.body` and land in the wrong window again.

import * as React from 'react'

/**
 * Anything that identifies the far side of a document boundary. A Window and a Document are
 * accepted so a boundary can hand over whatever it happens to hold.
 */
export type PortalContainerSource = Element | Document | Window | null | undefined

/**
 * What Base UI's portal accepts for `container`. Kept structurally identical to its own prop type
 * so an overlay can forward the value with no cast.
 */
export type PortalContainerValue =
  | HTMLElement
  | ShadowRoot
  | null
  | React.RefObject<HTMLElement | ShadowRoot | null>
  | undefined

/** `undefined` = "no override"; Base UI applies its own default. */
const PortalContainerContext = React.createContext<HTMLElement | undefined>(undefined)

/**
 * Duck-typed document resolution. Deliberately NOT `instanceof`: a node from a popup window belongs
 * to a DIFFERENT realm, so `el instanceof HTMLElement` is FALSE for it in the opener's realm. That
 * realm trap is the reason this whole module exists; re-introducing it here would be quietly fatal.
 */
function documentOf(source: PortalContainerSource): Document | undefined {
  if (source == null || typeof source !== 'object') return undefined
  // A Window is its own `window`, and owns a `document`.
  const asWindow = source as Window
  if (asWindow.window === asWindow && asWindow.document != null) return asWindow.document
  // A Document is the only Node whose `ownerDocument` is null.
  const asNode = source as Node
  if (asNode.ownerDocument === null && (source as Document).body !== undefined) {
    return source as Document
  }
  return asNode.ownerDocument ?? undefined
}

/**
 * Derive the portal target for `source`: that document's `<body>`, or `undefined` when it is the
 * ambient document (so the main-window path stays on Base UI's own default). Exported because it is
 * the whole behavioural claim of this module and is asserted directly by its tests.
 */
export function resolvePortalContainer(source: PortalContainerSource): HTMLElement | undefined {
  const doc = documentOf(source)
  if (doc == null) return undefined
  if (typeof document !== 'undefined' && doc === document) return undefined
  return doc.body ?? undefined
}

export interface PortalContainerProviderProps {
  /**
   * A handle to the document overlays below this point belong in — an Element inside it, the
   * Document, or its Window. Anything resolving to the ambient document is a no-op.
   */
  container?: PortalContainerSource
  children?: React.ReactNode
}

/**
 * Install at a cross-document boundary. Everything rendered below portals its overlays into
 * `container`'s document instead of the opener's.
 */
export function PortalContainerProvider({ container, children }: PortalContainerProviderProps) {
  const value = React.useMemo(() => resolvePortalContainer(container), [container])
  return <PortalContainerContext.Provider value={value}>{children}</PortalContainerContext.Provider>
}

/**
 * Hand descendants back to Base UI's own portal-node inheritance. Rendered by an overlay INSIDE its
 * portal, so a nested overlay portals into that overlay's portal node rather than beside it.
 */
export function PortalContainerInherit({ children }: { children?: React.ReactNode }) {
  return <PortalContainerContext.Provider value={undefined}>{children}</PortalContainerContext.Provider>
}

/**
 * The container an overlay should portal into. `explicit` (an overlay's own `container` prop) always
 * wins and is forwarded unchanged.
 */
export function usePortalContainer(explicit?: PortalContainerValue): PortalContainerValue {
  const inherited = React.useContext(PortalContainerContext)
  return explicit !== undefined ? explicit : inherited
}

/**
 * Element-only variant, for portals that are not Base UI's and so accept no ref form — the vaul
 * Drawer's Radix portal is the one such case in the kit.
 */
export function usePortalContainerElement(
  explicit?: HTMLElement | null,
): HTMLElement | null | undefined {
  const inherited = React.useContext(PortalContainerContext)
  return explicit !== undefined ? explicit : inherited
}
