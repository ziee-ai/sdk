// Browsers strip inner control/whitespace chars before parsing a URL, so a value
// like "java[TAB]script:alert(1)" still executes. Strip those chars first, then
// allowlist safe schemes. A URL with no scheme is treated as relative and allowed.
const SAFE_SCHEMES = ['http:', 'https:', 'mailto:', 'tel:']

export function safeHref(href: string): string | undefined {
  const cleaned = href.replace(/[\u0000-\u0020]/g, '')
  const scheme = cleaned.match(/^([a-z][a-z0-9+.-]*:)/i)
  // protocol-relative (//host, \\host) navigates off-site despite having no scheme — reject.
  if (!scheme) return /^[/\\]{2}/.test(cleaned) ? undefined : href
  return SAFE_SCHEMES.includes(scheme[1].toLowerCase()) ? href : undefined
}

// For <img src>: allow http(s) and data:image/* (raster/SVG-as-image is non-scripting),
// reject javascript:/data:text/html and other schemes. No scheme -> relative, allowed.
const SAFE_IMG_SCHEMES = ['http:', 'https:']
export function safeImgSrc(src: string): string | undefined {
  const cleaned = src.replace(/[\u0000-\u0020]/g, '')
  const scheme = cleaned.match(/^([a-z][a-z0-9+.-]*:)/i)
  if (!scheme) return src
  const s = scheme[1].toLowerCase()
  if (SAFE_IMG_SCHEMES.includes(s)) return src
  if (s === 'data:' && /^data:image\//i.test(cleaned)) return src
  return undefined
}
