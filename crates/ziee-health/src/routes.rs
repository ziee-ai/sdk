// Health routes configuration

use aide::axum::{ApiRouter, routing::get_with};

use super::handlers::*;

pub fn routes() -> ApiRouter {
    ApiRouter::new().api_route("/health", get_with(health_check, health_check_docs))
}
