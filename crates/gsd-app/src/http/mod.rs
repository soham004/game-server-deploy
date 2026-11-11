//! The HTTP layer: router assembly and middleware.
//!
//! Handlers are thin (parse → call one service → map the result); see
//! `claude/plan/01-architecture.md` §3. API routes live under `/api/v1`; everything else is
//! the embedded web UI. Every request needs a session except the few `access.rs` lets through.

pub mod access;
pub mod authorize;
pub mod client;
pub mod csrf;
pub mod dto;
pub mod error;
pub mod headers;
pub mod routes;
pub mod session;
pub mod throttle;
pub mod ws;

use axum::{Router, middleware};
use tower_http::{
    compression::{
        CompressionLayer,
        predicate::{DefaultPredicate, NotForContentType, Predicate},
    },
    trace::TraceLayer,
};

use crate::{assets, state::AppState};

/// The complete application router.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .merge(routes::system::routes())
        .merge(routes::setup::routes())
        .merge(routes::auth::routes())
        .merge(routes::me::routes())
        .merge(routes::catalog::routes())
        .merge(routes::instances::routes())
        .merge(routes::properties::routes())
        .merge(routes::files::routes())
        .merge(routes::backups::routes())
        .merge(routes::schedules::routes())
        .merge(routes::content::routes())
        .merge(routes::settings::routes())
        .merge(routes::app_settings::routes())
        .merge(routes::imports::routes())
        .merge(routes::modpacks::routes())
        .merge(routes::logs::routes())
        .merge(routes::control::routes())
        .merge(routes::players::routes())
        .merge(routes::gamerules::routes())
        .merge(routes::commands::routes())
        .merge(routes::worlds::routes())
        .merge(routes::users::routes())
        .merge(routes::jobs::routes())
        .merge(routes::java::routes())
        .merge(ws::routes())
        .fallback(routes::not_found)
        .method_not_allowed_fallback(routes::method_not_allowed)
        .layer(middleware::from_fn(csrf::require_csrf_header));

    Router::new()
        .nest("/api/v1", api)
        .fallback(assets::serve)
        // Before anything else: no session, no route (see `access.rs`).
        .layer(middleware::from_fn_with_state(
            state.clone(),
            access::require_session,
        ))
        .layer(middleware::from_fn(headers::add_security_headers))
        // WHY: downloads (`application/octet-stream`) go out as they are: jars and archives don't
        // compress further, and an unencoded file keeps its Content-Length (download progress).
        .layer(CompressionLayer::new().compress_when(
            DefaultPredicate::new().and(NotForContentType::const_new("application/octet-stream")),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
