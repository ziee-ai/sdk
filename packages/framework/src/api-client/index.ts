// @ziee/framework/api-client — the ApiClient transport RUNTIME.
//
// The generated per-endpoint types (`ApiEndpoints`, `ApiEndpointParameters`,
// `ApiEndpointResponses`) stay per-app OUTPUT (each app's generated
// `api-client/types.ts`). This module ships the transport (auth token, silent
// 401-refresh, retry, SSE, file-upload progress) + a generic `createApiClient`
// factory; the app binds its concrete types by casting the factory result to
// its own `ApiClientType` mapped type.
import { callAsync } from './core'

export {
  callAsync,
  getAuthToken,
  getBaseUrl,
  setBaseUrlResolver,
  setUnauthorizedHandler,
} from './core'
export type { FileUploadProgressCallback } from './core'
export { createSSEHandler } from './sse-types'
export type {
  SSECallback,
  SSEHandlers,
  SSEEventKey,
  SSEEventData,
} from './sse-types'

/**
 * Build the namespaced ApiClient from the app's generated `ApiEndpoints` map
 * (`"NS.method"` → `"METHOD /url"`). Groups the endpoint keys by namespace and
 * dispatches each method through the shared `callAsync` transport.
 *
 * Generic over the concrete client type: the per-endpoint parameter/response
 * typing lives in the app-side `ApiClientType` mapped type (over that app's
 * generated types), which the caller passes as `TClient`. The runtime itself is
 * type-erased — identical to the pre-extraction `createApiClient()` loop.
 */
export function createApiClient<TClient>(
  endpoints: Record<string, string>,
): TClient {
  const client = {} as any

  // Get all endpoint keys and group by namespace
  const endpointKeys = Object.keys(endpoints)

  endpointKeys.forEach(endpointKey => {
    const [namespace, method] = endpointKey.split('.') as [string, string]

    if (!client[namespace]) {
      client[namespace] = {}
    }

    // Create the method that calls callAsync with proper typing
    client[namespace][method] = async (params: any, callbacks?: any) => {
      return callAsync(endpoints[endpointKey], params, callbacks)
    }
  })

  return client as TClient
}
