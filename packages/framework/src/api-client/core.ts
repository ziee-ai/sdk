import type { SSECallback } from './sse-types'
import { createSSEHandler } from './sse-types'
import { getSyncConnectionId } from '../sync/connection'
import { netRequestEnd, netRequestStart } from '../net-idle'
import { bumpFetchEpoch, coalesce, inflightKey } from './inflight'

// ─────────────────────── Base-URL resolver (injected) ───────────────────────
//
// The base URL is app/platform-specific (browser same-origin vs the desktop
// Tauri dynamic port), so the framework transport takes it as an injection
// point — the same pattern as `setUnauthorizedHandler` below and the sync
// `initSync(authStore)` DI. The app registers its resolver at boot via
// `setBaseUrlResolver`; the default is the browser same-origin so an app that
// registers nothing still works on the web.
let baseUrlResolver: () => Promise<string> = () =>
  Promise.resolve(window.location.origin)

/** Register the app's base-URL resolver (called once at app boot). */
export const setBaseUrlResolver = (fn: () => Promise<string>): void => {
  baseUrlResolver = fn
}

/** Resolve the API base URL via the registered resolver (default: same-origin). */
export const getBaseUrl = (): Promise<string> => baseUrlResolver()

// ─────────────────────── Auth-token provider (injectable) ───────────────────
//
// The bearer token attached to every request is app/platform-specific — an app
// might hold it in a zustand-`persist` store, an httpOnly-cookie mirror, a
// desktop keychain, or a plain variable. The framework transport therefore
// takes the token via an injection point (the same DI pattern as
// `setBaseUrlResolver` / `setUnauthorizedHandler` above).
//
// DEFAULT (unchanged): read `localStorage['auth-storage']` with the
// `{ state: { token } }` zustand-persist shape — so ziee (and any app already
// relying on that contract) keeps working with NO code change. An app that
// stores its token elsewhere registers a provider via `setAuthTokenProvider`
// (or the `setAuthToken` convenience) and its requests then authenticate; this
// closes the silent-unauthenticated foot-gun where a non-`auth-storage` app
// invisibly sent every request without a bearer token.

let authTokenProvider: (() => string | null) | null = null

/**
 * Register a custom auth-token provider. When set, `getAuthToken()` calls this
 * function on every request instead of reading `localStorage['auth-storage']`.
 * Pass `null` to restore the default localStorage behavior.
 */
export const setAuthTokenProvider = (
  fn: (() => string | null) | null,
): void => {
  authTokenProvider = fn
}

/**
 * Convenience: authenticate with a static bearer token (or clear it with
 * `null`). Installs a provider that always returns the given value — the
 * common case for an app that fetches/refreshes its token out of band and
 * just wants the transport to use it.
 */
export const setAuthToken = (token: string | null): void => {
  authTokenProvider = () => token
}

/** The default token source: `localStorage['auth-storage']` (zustand-persist
 *  `{ state: { token } }` shape). Used when no provider is injected. */
const defaultAuthToken = (): string | null => {
  // eslint-disable-next-line no-undef
  const authData =
    typeof localStorage !== 'undefined'
      ? localStorage.getItem('auth-storage')
      : null
  if (!authData) return null
  try {
    const parsed = JSON.parse(authData)
    return parsed.state?.token || null
  } catch (error) {
    // Corrupt localStorage (manual edit, power-loss mid-write, schema
    // change from an older app version). Don't crash every API call —
    // treat as logged-out; user will be redirected to login.
    console.error('[getAuthToken] Failed to parse auth-storage:', error)
    return null
  }
}

/** Resolve the current bearer token via the injected provider if one is
 *  registered, else the default `localStorage['auth-storage']` source. */
export const getAuthToken = (): string | null => {
  if (authTokenProvider) return authTokenProvider()
  return defaultAuthToken()
}

// ─────────────────────── Silent-refresh integration ───────────────────────
//
// Module-level holder for the auth store's refreshSession, mirroring the
// sync connection-id holder (core/sync/connection.ts) to avoid an import
// cycle: the Auth store imports the api-client, so the api-client cannot
// import the Auth store back — it registers itself here in its __init__.

let onUnauthorized: (() => Promise<boolean>) | null = null

/** Register (or clear) the handler `callAsync` awaits on a 401 before
 *  retrying the request once with the refreshed token. Returns true when
 *  a fresh token landed. */
export const setUnauthorizedHandler = (
  handler: (() => Promise<boolean>) | null,
): void => {
  onUnauthorized = handler
}

// Endpoints where a 401 is a NORMAL outcome (bad credentials, revoked
// refresh token, wrong link password, …) — never trigger a silent
// refresh + retry for these, or a failed login would burn a rotation
// (and Auth.refresh itself would recurse).
const AUTH_REFRESH_EXEMPT = new Set<string>([
  'POST /api/auth/login',
  'POST /api/auth/register',
  'POST /api/auth/refresh',
  'POST /api/auth/logout',
  'POST /api/auth/link-account',
  'POST /api/auth/login-password-only',
  'POST /api/auth/magic-link/exchange',
  'POST /api/app/setup/admin',
  // Change-password returns 401 INVALID_CREDENTIALS on a wrong CURRENT
  // password — a normal outcome, not an expired session. Exempt so a
  // wrong-password attempt doesn't burn a refresh rotation and silently
  // re-submit the mutation.
  'POST /api/auth/password',
])

// Token-minting endpoints where a BROWSER client opts in to cookie-mode
// delivery (`X-Refresh-Cookie: 1` → the refresh token arrives as an
// httpOnly Set-Cookie and the JSON body's copy is blanked). Desktop
// Tauri windows keep body tokens (no header); note the served-over-
// tunnel phone pages hit desktop-only endpoints that are not listed
// here, and `/api/auth/refresh` obeys body-in→body-out regardless.
const COOKIE_MODE_ENDPOINTS = new Set<string>([
  'POST /api/auth/login',
  'POST /api/auth/register',
  'POST /api/auth/refresh',
  'POST /api/auth/link-account',
  'POST /api/app/setup/admin',
])

const isTauri = (): boolean =>
  typeof window !== 'undefined' &&
  Boolean((window as unknown as { __TAURI__?: unknown }).__TAURI__)

// Files upload progress callback type
export interface FileUploadProgressCallback {
  __init?: (xhr: XMLHttpRequest) => void
  onProgress?: (
    progress: number,
    fileIndex: number,
    overallProgress: number,
  ) => void
  onComplete?: (response: any) => void
  onError?: (error: string, fileName?: string) => void
}

// Transport runtime. Generic over the response type: the per-endpoint
// parameter/response typing is bound by the app-side ApiClient factory
// (which maps its generated ApiEndpoints → typed namespace methods), so the
// framework transport only needs the URL as a string and the response as a
// type parameter. `endpointUrl` stays a plain string here; the app's factory
// supplies the exact `"NS.method"` → URL values.
export const callAsync = <TResponse = unknown>(
  endpointUrl: string,
  params: any,
  callbacks?: {
    SSE?: SSECallback<TResponse>
    fileUploadProgress?: FileUploadProgressCallback
  },
): Promise<TResponse> => {
  // ── In-flight GET coalescing (see ./inflight.ts) ──────────────────────────
  // Only plain reads are joinable. Excluded, deliberately:
  //   - non-GET  — a mutation must never be silently collapsed into another's;
  //   - SSE      — a long-lived stream has ONE consumer wired to ONE reader, so
  //                two subscribers sharing a response body would starve one;
  //   - FormData — an upload is a mutation and carries non-comparable payloads.
  // The key is the endpoint template + its params (the resolved path/query is a
  // pure function of those), folded with the caller's identity.
  const isGet = endpointUrl.startsWith('GET ')
  const joinable =
    isGet && !callbacks?.SSE && !(params instanceof FormData)
  if (!joinable) return performCall<TResponse>(endpointUrl, params, callbacks)

  const key = inflightKey('GET', `${endpointUrl}|${stableParams(params)}`, getAuthToken())
  return coalesce(key, () => performCall<TResponse>(endpointUrl, params, callbacks))
}

/**
 * Deterministic, order-independent serialization of a params object for the
 * coalescing key. Two callers that pass equivalent params must produce the same
 * key regardless of literal key order; `undefined` values are dropped because
 * the URL builder drops them too.
 */
const stableParams = (params: any): string => {
  if (params === undefined || params === null) return ''
  if (typeof params !== 'object') return String(params)
  try {
    const entries = Object.entries(params)
      .filter(([, v]) => v !== undefined)
      .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    return JSON.stringify(entries)
  } catch {
    // Non-serializable params (a cyclic object) — fall back to a key that never
    // matches another call, so such a request is simply never coalesced.
    return `nokey:${Math.random()}`
  }
}

const performCall = async <TResponse = unknown>(
  endpointUrl: string,
  params: any,
  callbacks?: {
    SSE?: SSECallback<TResponse>
    fileUploadProgress?: FileUploadProgressCallback
  },
): Promise<TResponse> => {
  let bUrl = await getBaseUrl()

  let { SSE: sseCallbacks, fileUploadProgress } = callbacks || {}

  // If SSE is an object (handlers), wrap it with createSSEHandler
  let sseFunction: ((event: string, data: any) => void) | undefined = undefined
  if (sseCallbacks) {
    if (typeof sseCallbacks === 'function') {
      sseFunction = sseCallbacks as (event: string, data: any) => void
    } else {
      sseFunction = createSSEHandler(sseCallbacks as any)
    }
  }

  // Track in-flight NON-SSE requests so the background action-chunk prefetch can
  // wait for the page's critical loads to finish (SSE streams are long-lived).
  const counted = !sseFunction
  if (counted) netRequestStart()

  try {
    // Check if params is FormData for file uploads
    const isFormData = params instanceof FormData

    let headers: Record<string, string> = {}

    // Don't set Content-Type for FormData - let browser set it with boundary
    if (!isFormData) {
      headers['Content-Type'] = 'application/json'
    }

    // Add auth token if available
    const token = getAuthToken()
    if (token) {
      headers['Authorization'] = `Bearer ${token}`
    }

    // Browser clients opt in to httpOnly-cookie refresh-token delivery
    // on the token-minting endpoints (see COOKIE_MODE_ENDPOINTS).
    if (!isTauri() && COOKIE_MODE_ENDPOINTS.has(endpointUrl)) {
      headers['X-Refresh-Cookie'] = '1'
    }

    const method = endpointUrl.split(' ')[0] as
      | 'POST'
      | 'GET'
      | 'PUT'
      | 'DELETE'
      | 'PATCH'

    // Stamp the originating realtime-sync connection id on mutating
    // requests so the server skips echoing the resulting change back to
    // this tab (self-echo suppression). Harmless when absent / on GETs.
    if (method !== 'GET') {
      const syncConnId = getSyncConnectionId()
      if (syncConnId) {
        headers['X-Sync-Connection-Id'] = syncConnId
      }
    }

    let endpointPath = endpointUrl.replace(/^[A-Z]+\s+/, '').trim()
    //get {capture} from endpointPath
    const captureMatches = (endpointPath.match(/{([^}]+)}/g) || []).map(match =>
      match.slice(1, -1),
    )

    // For FormData, we need to handle path parameters differently
    if (isFormData) {
      // Replace {capture} with actual values from FormData entries
      captureMatches.forEach(capture => {
        const value = (params as FormData).get(capture.trim())
        if (value !== null) {
          endpointPath = endpointPath.replace(`{${capture}}`, value.toString())
        } else {
          throw new Error(`Missing required parameter: ${capture}`)
        }
      })
    } else {
      // Replace {capture} with actual values from params object
      captureMatches.forEach(capture => {
        let c = capture.trim() as keyof typeof params
        if (params[c] !== undefined) {
          //@ts-ignore
          endpointPath = endpointPath.replace(`{${capture}}`, params[c])
        } else {
          throw new Error(`Missing required parameter: ${capture}`)
        }
      })
    }

    if (method === 'GET') {
      //add query parameters to the URL for GET requests
      const queryParams: string[] = []
      if (params && typeof params === 'object') {
        for (const [key, value] of Object.entries(params)) {
          if (value !== undefined && !captureMatches.includes(key)) {
            // Encode the key and value to ensure they are URL-safe
            queryParams.push(
              `${encodeURIComponent(key)}=${encodeURIComponent(String(value))}`,
            )
          }
        }
        if (queryParams.length > 0) {
          endpointPath += `?${queryParams.join('&')}`
        }
      }
    }

    // Prepare the request body. Strip path-captured params from the
    // body — sending them as body fields (e.g. `provider_id`) trips
    // the backend's serde deny-unknown-fields validators (introduced
    // in the 2026-05 security pass), which return 422.
    let body: any = undefined
    if (['POST', 'PUT', 'PATCH'].includes(method) && params !== undefined) {
      if (isFormData) {
        body = params as FormData
      } else if (captureMatches.length > 0 && typeof params === 'object') {
        const bodyParams: Record<string, unknown> = {}
        for (const [key, value] of Object.entries(params)) {
          if (!captureMatches.includes(key)) {
            bodyParams[key] = value
          }
        }
        body = JSON.stringify(bodyParams)
      } else {
        body = JSON.stringify(params)
      }
    }

    let response: Response
    let abortController: AbortController | undefined = undefined

    // Use XMLHttpRequest for FormData uploads with progress tracking
    if (isFormData && fileUploadProgress && body) {
      response = await new Promise<Response>((resolve, reject) => {
        const xhr = new XMLHttpRequest()

        // Call __init callback if provided to give caller access to XHR for cancellation
        fileUploadProgress.__init?.(xhr)

        // Get file information from FormData
        const formData = body as FormData
        const files: { name: string; file: File; size: number }[] = []

        // Extract files from FormData for progress tracking
        for (const [key, value] of formData.entries()) {
          if (value instanceof File) {
            files.push({ name: key, file: value, size: value.size })
          }
        }

        // Calculate cumulative file sizes for progress tracking
        const fileCumsums: {
          name: string
          startByte: number
          endByte: number
          size: number
        }[] = []
        let cumulativeSize = 0

        files.forEach(fileInfo => {
          const startByte = cumulativeSize
          const endByte = cumulativeSize + fileInfo.size
          fileCumsums.push({
            name: fileInfo.name,
            startByte,
            endByte,
            size: fileInfo.size,
          })
          cumulativeSize += fileInfo.size
        })

        let lastReportedFileIndex = -1

        xhr.upload.addEventListener('progress', event => {
          if (event.lengthComputable) {
            const bytesUploaded = event.loaded
            const totalBytes = event.total
            const overallProgress = Math.round(
              (bytesUploaded / totalBytes) * 100,
            )

            // Find which file is currently being uploaded
            let currentFileIndex = 0
            let fileProgress = 0

            for (let i = 0; i < fileCumsums.length; i++) {
              const fileInfo = fileCumsums[i]

              if (
                bytesUploaded >= fileInfo.startByte &&
                bytesUploaded <= fileInfo.endByte
              ) {
                currentFileIndex = i

                // Calculate progress within this specific file
                const bytesUploadedInFile = bytesUploaded - fileInfo.startByte
                fileProgress = Math.round(
                  (bytesUploadedInFile / fileInfo.size) * 100,
                )
                break
              } else if (bytesUploaded > fileInfo.endByte) {
                // This file is completely uploaded
                currentFileIndex = i
                fileProgress = 100
              }
            }

            // Report progress for all completed files that weren't reported yet
            for (let i = lastReportedFileIndex + 1; i < currentFileIndex; i++) {
              fileUploadProgress.onProgress?.(100, i, overallProgress)
            }

            // Report progress for the current file being uploaded
            if (currentFileIndex >= lastReportedFileIndex) {
              fileUploadProgress.onProgress?.(
                fileProgress,
                currentFileIndex,
                overallProgress,
              )
              lastReportedFileIndex = currentFileIndex
            }
          }
        })

        xhr.addEventListener('load', () => {
          if (xhr.status >= 200 && xhr.status < 300) {
            // Call onComplete callback if provided
            try {
              const responseData = JSON.parse(xhr.responseText)
              fileUploadProgress.onComplete?.(responseData)
            } catch {
              // If response is not JSON, call onComplete with the text
              fileUploadProgress.onComplete?.(xhr.responseText)
            }

            // Create a Response-like object
            const responseHeaders = new Headers()
            xhr
              .getAllResponseHeaders()
              .split('\r\n')
              .forEach(header => {
                const [key, value] = header.split(': ')
                if (key && value) {
                  responseHeaders.set(key, value)
                }
              })

            const mockResponse = {
              ok: xhr.status >= 200 && xhr.status < 300,
              status: xhr.status,
              statusText: xhr.statusText,
              headers: responseHeaders,
              text: () => Promise.resolve(xhr.responseText),
              json: () => Promise.resolve(JSON.parse(xhr.responseText)),
            } as Response

            resolve(mockResponse)
          } else {
            reject(new Error(`HTTP error! status: ${xhr.status}`))
          }
        })

        xhr.addEventListener('error', () => {
          fileUploadProgress.onError?.('Network error during file upload')
          reject(new Error('Network error during file upload'))
        })

        xhr.addEventListener('abort', () => {
          fileUploadProgress.onError?.('Upload cancelled')
          reject(new Error('Upload cancelled'))
        })

        xhr.open(method, `${bUrl}${endpointPath}`)

        // Set headers (excluding Content-Type for FormData)
        Object.entries(headers).forEach(([key, value]) => {
          if (key !== 'Content-Type') {
            xhr.setRequestHeader(key, value)
          }
        })

        xhr.send(body)
      })
    } else {
      // Create AbortController for SSE stream management if SSE callbacks are provided
      abortController = sseFunction ? new AbortController() : undefined

      // Use fetch for non-FormData requests or when no progress tracking is needed.
      //
      // Transient network failures (connection refused/reset, DNS blip) reject
      // fetch() with a TypeError — no HTTP response was ever received, so the
      // server never processed the request. For idempotent GETs that aren't SSE
      // streams, retry a few times with backoff rather than surfacing the error:
      // a momentary blip (server briefly busy, many tabs/devices cold-loading at
      // once) must not break a data load or, for /auth/me-style calls, brick the
      // app. Never retried: non-GET (could double-submit), SSE streams (their own
      // reconnect logic), AbortError (intentional cancel), or an HTTP error
      // RESPONSE (the server did respond — handled by the status logic below).
      // The cold-load GETs all fire in parallel and each retries independently,
      // so the budget below is wall-clock ~6s for the whole burst, not per call.
      const retryableGet = method === 'GET' && !sseFunction
      for (let attempt = 0; ; attempt++) {
        try {
          response = await fetch(`${bUrl}${endpointPath}`, {
            method,
            headers,
            body,
            signal: abortController?.signal,
          })
          break
        } catch (err) {
          if (err instanceof Error && err.name === 'AbortError') throw err
          if (!retryableGet || attempt >= 5) throw err
          await new Promise(r => setTimeout(r, 200 * 2 ** attempt))
        }
      }

      // Silent-refresh on 401: the access token expired mid-session
      // (e.g. laptop woke from sleep past exp). Ask the auth store to
      // rotate it via /api/auth/refresh, then retry the request ONCE
      // with the fresh token. Safe for the SSE branch too — at this
      // point no stream bytes have been consumed. Skipped for the auth
      // endpoints where 401 is a normal outcome, and when no handler is
      // registered (auth store not initialized yet). A false return
      // (refresh failed → session cleared) falls through to the normal
      // 401 error handling below.
      if (
        response.status === 401 &&
        onUnauthorized &&
        !AUTH_REFRESH_EXEMPT.has(endpointUrl)
      ) {
        // The handler (Auth.store.refreshSession) is internally guarded,
        // but its navigator.locks.request wrapper can itself reject in
        // edge webview states — never let that replace the original 401.
        const refreshed = await onUnauthorized().catch(() => false)
        if (refreshed) {
          const freshToken = getAuthToken()
          if (freshToken) {
            headers['Authorization'] = `Bearer ${freshToken}`
          }
          response = await fetch(`${bUrl}${endpointPath}`, {
            method,
            headers,
            body,
            signal: abortController?.signal,
          })
        }
      }

      // Send initial __init event with abortController for SSE streams
      if (abortController && sseFunction) {
        sseFunction('__init' as any, { abortController })
      }
    }

    // Handle SSE streaming if callbacks are provided and response is text/event-stream
    if (
      sseFunction &&
      response.headers.get('Content-Type')?.includes('text/event-stream')
    ) {
      if (!response.ok) {
        const errorMessage = `HTTP error! status: ${response.status}`
        const error = new Error(errorMessage) as Error & { status: number }
        error.status = response.status
        throw error
      }

      const reader = response.body?.getReader()
      if (!reader) {
        const error = 'No response body reader available'
        throw new Error(error)
      }

      const decoder = new globalThis.TextDecoder()
      let buffer = ''

      try {
        let currentEvent = ''

        while (true) {
          // Check if abort was requested
          if (abortController?.signal.aborted) {
            reader.releaseLock()
            break
          }

          const { done, value } = await reader.read()
          if (done) break

          buffer += decoder.decode(value, { stream: true })
          const lines = buffer.split('\n')
          buffer = lines.pop() || '' // Keep incomplete line in buffer

          for (const line of lines) {
            if (line.trim() === '') {
              // Empty line indicates end of event, reset current event
              currentEvent = ''
              continue
            }

            if (line.startsWith('event: ')) {
              currentEvent = line.slice(7).trim()
            } else if (line.startsWith('data: ')) {
              const data = line.slice(6)
              let parsed = data

              try {
                parsed = JSON.parse(data)
              } catch {
                //do nothing, keep as string
              }

              const result = sseFunction?.(currentEvent as any, parsed as any)
              if ((result as unknown) instanceof Promise) await result
            }
          }
        }
      } catch (error) {
        reader.releaseLock()
        throw error
      }

      // For SSE streaming, return empty response since data is handled via callbacks
      return {} as TResponse
    }

    // Parse the response as JSON
    if (!response.ok) {
      let errorMessage = `HTTP error! status: ${response.status}`
      let errorCode: string | undefined

      // Handle 403 Forbidden specifically
      if (response.status === 403) {
        try {
          // Try to extract specific error message from response body
          const errorResponse = await response.json()
          if (errorResponse.error) {
            errorMessage = errorResponse.error
          } else {
            errorMessage = 'Permission denied'
          }
        } catch {
          // If we can't parse the error response, use default permission denied message
          errorMessage = 'Permission denied'
        }
      } else {
        let textResponse: string | undefined
        try {
          textResponse = await response.text()
        } catch {
          textResponse = ''
        }
        try {
          // Try to extract error message from response body for other errors
          const errorResponse = JSON.parse(textResponse)
          if (errorResponse.error) {
            errorMessage = errorResponse.error
          }
          if (errorResponse.error_code) {
            errorCode = errorResponse.error_code
          }
        } catch {
          // If we can't parse the error response, use the default message
          // get the reponse text instead
          if (textResponse) {
            errorMessage = `HTTP error! status: ${response.status} - ${textResponse}`
          }
        }
      }

      // Attach both the HTTP status AND the stable machine-readable
      // error_code so a caller's `catch` can branch on either without
      // parsing the message. A plain `Error` has neither field so a
      // `'status' in e` / `'error_code' in e` check would silently
      // never match without this.
      const error = new Error(errorMessage) as Error & {
        status: number
        error_code?: string
      }
      error.status = response.status
      if (errorCode) {
        error.error_code = errorCode
      }
      throw error
    }

    //try to parse the response based on content type
    const contentType = response.headers.get('Content-Type') || ''

    if (contentType.includes('application/json')) {
      return (await response.json()) as TResponse
    } else if (
      contentType.startsWith('text/') ||
      contentType.includes('application/xml') ||
      contentType.includes('application/javascript')
    ) {
      // Return text for text-like content types
      const textResponse = await response.text()
      return textResponse as unknown as TResponse
    } else {
      // Default to binary (Blob) for everything else. Was previously
      // a whitelist of image/video/audio/pdf/octet-stream with a
      // text fallback for unknowns — but that fallback corrupted
      // any binary file whose MIME type wasn't in the list (XLSX,
      // ZIP, MS Office docs, custom application/* types). UTF-8
      // decoding replaces every non-valid byte sequence with U+FFFD
      // (3 bytes each), and the subsequent xlsx/zip parser sees
      // garbage and OOMs trying to allocate based on mis-read
      // section lengths. Binary-as-default preserves bytes; the
      // explicit text branch above still handles legitimate text
      // responses.
      const blobResponse = await response.blob()
      return blobResponse as unknown as TResponse
    }
  } catch (error) {
    // Handle AbortErrors more gracefully - they're expected during cleanup
    if (error instanceof Error && error.name === 'AbortError') {
      console.log(`Request to ${endpointUrl} was aborted`)
    } else {
      console.error(`Error calling endpoint ${endpointUrl}:`, error)
    }
    throw error // Re-throw to allow caller to handle it
  } finally {
    if (counted) netRequestEnd()
    // A completed MUTATION invalidates joinability of every read that was
    // already on the wire — a refetch issued after this point must get its own
    // round-trip rather than joining a possibly-pre-mutation request. Bumped on
    // failure too: a request that errored client-side may still have been
    // applied server-side, so assuming "nothing changed" would be unsafe.
    if (!endpointUrl.startsWith('GET ')) bumpFetchEpoch()
  }
}
