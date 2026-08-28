// The WIDTH rule every option-list popup wears, as one string.
//
// ── What it says ─────────────────────────────────────────────────────────────────────────
//   as wide as it needs · never narrower than the trigger · never wider than the space there is
//
// ── Why it is a default and not a prop ───────────────────────────────────────────────────
// It used to be a prop. `Select` took `popupMatchSelectWidth` (default `true` — pinned to the
// trigger) and every call site that wanted a readable list had to remember to pass `false`.
// Four did; every other option list in the kit — the base Select popup, the Combobox, the
// dropdown Menu, and `MultiSelect`, which had no knob at all — stayed pinned. That is the
// failure mode of an opt-in: it is missing on the next control someone adds, and nothing says
// so. So the rule moved here, the knob was deleted, and the four opt-outs went with it.
//
// ── The three parts, and why none is optional ────────────────────────────────────────────
//
// `w-auto` — the popup shrink-wraps its widest row instead of inheriting `--anchor-width`.
//   This is the reported defect: a trigger in a narrow form column or a docked toolbar clipped
//   the very option text the list was opened to read.
//
// `min-w-(--anchor-width)` — the FLOOR. Content-sizing alone would let a list of three short
//   words render narrower than the control it dropped out of, which reads as a rendering fault
//   rather than as a fit. (`min-width` beats `max-width` in CSS, so a trigger wider than the
//   cap below still gets a popup as wide as itself — deliberately.)
//
// `max-w-[min(var(--available-width,100vw),32rem)]` — the CEILING, and it is TWO bounds:
//
//   · `--available-width` is published by Base UI's positioner from floating-ui's `size`
//     middleware, measured against the clipping boundary OF THE POPUP'S OWN DOCUMENT. That
//     matters: a pane popped out of the workbench renders into a second window, and a popup
//     opened there must be bounded by THAT window, not by the opener behind it. Because the
//     bound is a CSS custom property resolved on the popup's own element, it is correct there
//     by construction — the `100vw` fallback (for the first frame, before the middleware has
//     run) resolves against the same window for the same reason. `shift` cannot rescue this:
//     it re-positions a misplaced popup, but a popup WIDER than the window has nowhere to go.
//
//   · `32rem` is a readable-line cap — the same width this design system already caps a form
//     column at. Without it, one pathological 400-character label would size the popup to the
//     whole monitor, which is its own defect. At the cap the ROW gives instead of the popup:
//     the list clips (each popup already owns an `overflow-x-hidden`), and a caller that wants
//     an ellipsis or a wrap says so on the content it renders — which the kit now lets it do,
//     see `SelectItem`'s `min-w-0` note.
//
// ── Composition ──────────────────────────────────────────────────────────────────────────
// Put this FIRST in a `cn()` so a component's own narrower rule (the Combobox's
// `min-w-[calc(var(--anchor-width)+…)]`, a caller's `className`) still wins — `twMerge` keeps
// the last of two conflicting utilities.
//
// That cuts both ways: a component that ALSO wants an absolute floor (the Select's old
// `min-w-36`, the Menu's `min-w-32`) must state BOTH terms in ONE utility —
// `min-w-[max(var(--anchor-width),--spacing(36))]` — because a bare `min-w-36` after this
// string would silently delete the trigger floor. The FLOOR test in
// `ui/tests/visual/picker-popup-width.spec.ts` is what catches that mistake.
export const optionListPopupWidth =
  'w-auto min-w-(--anchor-width) max-w-[min(var(--available-width,100vw),32rem)]'
