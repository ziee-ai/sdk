//! `NotificationModule` — the self-registering AppModule (turnkey `module`
//! feature). Mounts the inbox routes + spawns the retention prune loop.

use std::error::Error;
use std::time::Duration;

use aide::axum::ApiRouter;
use linkme::distributed_slice;
use sqlx::PgPool;

use ziee_auth::auth::DefaultIdentityResolver;
use ziee_framework::{AppModule, ModuleContext, ModuleEntry, MODULE_ENTRIES};

/// Fixed retention (0 = keep forever). A promotable const, not a bare magic
/// number — an app with an admin scheduler-settings surface can drive this from
/// a settings row later (DEC-17: CytoAnalyst has no such surface yet).
const NOTIFICATION_RETENTION_DAYS: i64 = 0;

/// ~6h prune cadence.
const PRUNE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[distributed_slice(MODULE_ENTRIES)]
static NOTIFICATION_MODULE_REGISTRATION: ModuleEntry = ModuleEntry {
    name: "notification",
    // After its own table exists (migrations run at build); no init-ordering
    // dependency on other modules.
    order: 12,
    description: "Durable notification inbox",
    constructor: || Box::new(NotificationModule),
};

pub struct NotificationModule;

impl AppModule for NotificationModule {
    fn name(&self) -> &'static str {
        "notification"
    }

    fn description(&self) -> &'static str {
        "Durable notification inbox"
    }

    fn init(&mut self, ctx: &ModuleContext) -> Result<(), Box<dyn Error>> {
        // Periodic retention prune. Fire-and-forget; no-op while retention <= 0.
        let pool = (*ctx.db_pool).clone();
        tokio::spawn(async move { run_prune_loop(pool).await });
        Ok(())
    }

    fn register_routes(&self, router: ApiRouter) -> ApiRouter {
        router.merge(crate::handlers::notification_router::<DefaultIdentityResolver>())
    }
}

async fn run_prune_loop(pool: PgPool) {
    let mut tick = tokio::time::interval(PRUNE_INTERVAL);
    loop {
        tick.tick().await;
        if NOTIFICATION_RETENTION_DAYS > 0 {
            let _ = crate::repository::prune_older_than(&pool, NOTIFICATION_RETENTION_DAYS).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_is_registered_in_the_slice() {
        let entry = MODULE_ENTRIES
            .iter()
            .find(|e| e.name == "notification")
            .expect("notification module self-registers into MODULE_ENTRIES");
        assert_eq!(entry.order, 12);
        assert_eq!(entry.description, "Durable notification inbox");
        // The registered constructor builds the concrete module.
        assert_eq!((entry.constructor)().name(), "notification");
    }

    #[test]
    fn app_module_impl_reports_identity() {
        let m = NotificationModule;
        assert_eq!(m.name(), "notification");
        assert_eq!(m.description(), "Durable notification inbox");
    }

    #[test]
    fn retention_defaults_to_keep_forever() {
        // DEC-17: no scheduler-settings surface yet, so the prune loop is a no-op.
        assert_eq!(NOTIFICATION_RETENTION_DAYS, 0);
    }
}
