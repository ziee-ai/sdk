# @ziee/kit

The ziee design-system component kit — shadcn/Base-UI components rendering
against a self-contained design-token layer.

## Styling — Tailwind v4 wiring (one line)

`@ziee/kit` is built for **Tailwind v4** (CSS-first; no `tailwind.config.js`).
Tailwind v4 only auto-scans your app's own source tree — it never scans
`node_modules` — so without wiring, the kit's components render **unstyled**
(Tailwind never sees their utility classes).

Wire it up with a single import in your app's main CSS, right after Tailwind
itself:

```css
@import "tailwindcss";
@import "@ziee/kit/styles/kit.css";
```

`kit.css` does both things the kit needs:

1. imports the design tokens (`@ziee/kit/styles/tokens.css` — the shadcn CSS
   variables the components use: `bg-primary`, `text-muted-foreground`, …), and
2. adds an `@source "…/@ziee/kit/src/**/*.{ts,tsx}"` directive (resolved
   relative to the kit, so it works from anywhere) telling Tailwind to scan the
   kit's own component sources so their classes make it into your generated CSS.

That's the whole wiring — you get styled kit components with one line.

### Advanced: tokens only

If you want ONLY the token layer (e.g. you already provide your own `@source`
glob, or you want to override tokens), import the tokens file directly and add
your own source directive:

```css
@import "tailwindcss";
@import "@ziee/kit/styles/tokens.css";
@source "../node_modules/@ziee/kit/src/**/*.{ts,tsx}";
```

> The `@source` path is resolved relative to the CSS file that contains it, so
> adjust the `../node_modules/…` prefix to match where your CSS lives relative
> to `node_modules`.

## Usage

```tsx
import { Button } from "@ziee/kit";

export function Example() {
  return <Button variant="default">Hello</Button>;
}
```
