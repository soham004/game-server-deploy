//! Unauthenticated system endpoints: health check and product metadata.

use axum::{Json, Router, routing::get};
use serde::Serialize;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/meta", get(meta))
}

#[derive(Debug, Serialize)]
struct Health {
    status: &'static str,
}

async fn health() -> Json<Health> {
    Json(Health { status: "ok" })
}

/// Branding for the frontend, which never hard-codes the product name.
#[derive(Debug, Serialize)]
struct Meta {
    name: &'static str,
    slug: &'static str,
    version: &'static str,
}

async fn meta() -> Json<Meta> {
    Json(Meta {
        name: gsd_brand::NAME,
        slug: gsd_brand::SLUG,
        version: gsd_brand::VERSION,
    })
}
