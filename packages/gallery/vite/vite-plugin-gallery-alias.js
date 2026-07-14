/**
 * Dev-server alias for the component gallery entry.
 *
 * The gallery's HTML entry is `src/gallery.html`, served by Vite at
 * `/gallery.html`. This middleware adds two conveniences (dev-only):
 *
 *   - `/gallery` (and `/gallery/`) → serves `/gallery.html` (the pretty URL).
 *   - `/dev-gallery.html`          → serves `/gallery.html` (backward-compat
 *                                    alias for the pre-rename path; every old
 *                                    bookmark / script keeps working).
 *
 * The rewrite preserves the query string (`?surface=&state=&theme=&dir=`), which
 * the gallery reads for single-surface + theme rendering. It only touches the
 * request PATHNAME, so Vite then resolves + serves `gallery.html` normally.
 */
export function galleryAliasPlugin() {
  const rewrite = (url) => {
    const qIndex = url.indexOf('?')
    const pathname = qIndex === -1 ? url : url.slice(0, qIndex)
    const search = qIndex === -1 ? '' : url.slice(qIndex)
    if (
      pathname === '/gallery' ||
      pathname === '/gallery/' ||
      pathname === '/dev-gallery.html'
    ) {
      return `/gallery.html${search}`
    }
    return null
  }
  const middleware = (req, _res, next) => {
    if (req.url) {
      const rewritten = rewrite(req.url)
      if (rewritten) req.url = rewritten
    }
    next()
  }
  return {
    name: 'gallery-alias',
    apply: 'serve',
    configureServer(server) {
      server.middlewares.use(middleware)
    },
    configurePreviewServer(server) {
      server.middlewares.use(middleware)
    },
  }
}
