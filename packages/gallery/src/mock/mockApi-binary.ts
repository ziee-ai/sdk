// Binary-response helper for the gallery mock-API.
//
// The gallery `mockApi` normally answers every `/api/*` route with
// `jsonResponse(...)`. A binary viewer (e.g. PDF.js) fetches raw *bytes*, so the
// interceptor needs a way to return a binary body. This tiny, dependency-/JSX-
// free module builds that Response so it can be unit-tested under `node --test`.

/** Build a binary `Response` (default 200) for a byte payload. */
export function makeBinaryResponse(
  bytes: Uint8Array,
  contentType: string,
  status = 200,
): Response {
  // Copy into a standalone ArrayBuffer so the Response owns its bytes.
  const buf = bytes.slice().buffer
  return new Response(buf, {
    status,
    headers: {
      'content-type': contentType,
      'content-length': String(bytes.byteLength),
    },
  })
}

/** Decode a base64 string to bytes (fixtures are stored base64 in TS). */
export function base64ToBytes(b64: string): Uint8Array {
  const binary = atob(b64)
  const out = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i)
  return out
}
