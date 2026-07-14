// Ambient declaration for side-effect CSS imports (e.g. the kit's scroll-area
// pulls in `overlayscrollbars/overlayscrollbars.css`). In the ziee app this is
// provided by `vite/client`; the kit is bundler-agnostic and declares it
// itself so a standalone `tsc --noEmit` resolves the import.
declare module '*.css';
