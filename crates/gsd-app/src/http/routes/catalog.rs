//! What can be installed (for the version picker).

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use gsd_minecraft::{
    LoaderKind,
    catalog::{GameVersion, LoaderAvailability, LoaderVersion},
};
use serde::Deserialize;

use crate::{
    http::{error::ApiError, session::CurrentActor},
    state::AppState,
};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/catalog/minecraft/versions", get(versions))
        .route("/catalog/minecraft/loaders", get(loaders))
        .route(
            "/catalog/minecraft/loaders/{loader}/versions",
            get(loader_versions),
        )
}

#[derive(Debug, Deserialize)]
struct McQuery {
    mc: String,
}

/// Every Minecraft version, newest first (the UI filters by kind).
async fn versions(
    State(state): State<AppState>,
    _actor: CurrentActor,
) -> Result<Json<Vec<GameVersion>>, ApiError> {
    Ok(Json(state.minecraft.catalog().game_versions().await?))
}

/// Which server software supports `?mc=`.
async fn loaders(
    State(state): State<AppState>,
    _actor: CurrentActor,
    Query(query): Query<McQuery>,
) -> Result<Json<Vec<LoaderAvailability>>, ApiError> {
    Ok(Json(state.minecraft.catalog().loaders_for(&query.mc).await?))
}

/// Versions of one loader for `?mc=`, newest first, with the recommended one marked.
async fn loader_versions(
    State(state): State<AppState>,
    _actor: CurrentActor,
    Path(loader): Path<LoaderKind>,
    Query(query): Query<McQuery>,
) -> Result<Json<Vec<LoaderVersion>>, ApiError> {
    Ok(Json(
        state
            .minecraft
            .catalog()
            .loader_versions(loader, &query.mc)
            .await?,
    ))
}
