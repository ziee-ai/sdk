import type { AppModule } from '../module-system/types'
import { createModule } from '../module'
import { useRoutesStore } from './routes-store'
import { RouterComponent } from './RouterComponent'
import { setRouterConfig, type RouterConfig } from './config'
import './types' // Enable the CreateModuleOptions.routes + Slots declaration-merge

// @ziee/framework/router — the router module factory.
//
// An app opts into SDK routing by registering the module this returns
// (alongside its feature modules). It renders first (`order: 0`), collects
// every module's `routes` via `onModuleRegister`, and mounts a
// `<BrowserRouter>` that renders them with layout/guard grouping.

export interface CreateRouterModuleOptions extends Partial<RouterConfig> {}

/**
 * Build the router module. Pass app-specific knobs (redirect paths, the
 * Suspense fallback, the permission gate) as options — all optional, sensible
 * defaults otherwise.
 *
 * @example
 *   registerModules([
 *     createRouterModule({ loginPath: '/login', fallback: <Spinner/> }),
 *     ...featureModules,
 *   ])
 */
export function createRouterModule(
  options: CreateRouterModuleOptions = {},
): AppModule {
  setRouterConfig(options)

  return createModule({
    metadata: {
      name: 'router',
      version: '1.0.0',
      description: 'Provides routing infrastructure and layout management',
    },

    dependencies: [], // Loads first.

    stores: [{ name: 'Routes', store: useRoutesStore }],

    components: [
      {
        id: 'router',
        // Render the router element directly (already a component, not lazy) so
        // the SDK stays free of any app-specific lazy-loading utility.
        component: <RouterComponent />,
        order: 0, // Render first.
      },
    ],

    routes: [], // The router registers no routes of its own.

    // Collect `routes` from every registered module into the Routes store.
    onModuleRegister: (module: AppModule) => {
      const withRoutes = module as AppModule & { routes?: unknown[] }
      if (withRoutes.routes && withRoutes.routes.length > 0) {
        useRoutesStore.getState().addRoutes(withRoutes.routes as any)
      }
    },
  })
}
