//! Typed permission checks for handlers.
//!
//! A handler declares what it needs in its signature and the check happens before it runs:
//!
//! ```ignore
//! async fn write_file(auth: Authorized<perm::FilesWrite>, …) { /* auth.actor, auth.instance_id */ }
//! async fn create_instance(auth: AuthorizedGlobal<perm::InstanceCreate>, …) { … }
//! ```
//!
//! Instance routes must name their path parameter `instance_id`. A user who cannot even view the
//! instance gets `404` (so instance IDs don't leak); one who can view but lacks the permission
//! gets `403 missing_permission`.

use std::marker::PhantomData;

use axum::{
    extract::{FromRequestParts, RawPathParams},
    http::{StatusCode, request::Parts},
};
use gsd_core::{
    auth::{Actor, GlobalPermission, Permission},
    ids::InstanceId,
};

use crate::{
    http::{error::ApiError, session::CurrentActor},
    state::AppState,
};

/// Marker type naming an instance permission (see [`perm`]).
pub trait InstancePermission {
    const PERMISSION: Permission;
}

/// Marker type naming a global permission (see [`perm`]).
pub trait GlobalPermissionMarker {
    const PERMISSION: GlobalPermission;
}

/// The actor, after checking they hold `P` on the instance in the path.
#[derive(Debug)]
pub struct Authorized<P> {
    pub actor: Actor,
    pub instance_id: InstanceId,
    _permission: PhantomData<P>,
}

/// The actor, after checking they hold the global permission `P`.
#[derive(Debug)]
pub struct AuthorizedGlobal<P> {
    pub actor: Actor,
    _permission: PhantomData<P>,
}

impl<P: InstancePermission + Send + Sync> FromRequestParts<AppState> for Authorized<P> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let CurrentActor(actor) = CurrentActor::from_request_parts(parts, state).await?;
        let instance_id = instance_id_from_path(parts, state).await?;

        if !actor.can(Permission::InstanceView, instance_id) {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "instance_not_found",
                "No such instance.",
            ));
        }
        if !actor.can(P::PERMISSION, instance_id) {
            return Err(missing_permission(&P::PERMISSION));
        }
        Ok(Self {
            actor,
            instance_id,
            _permission: PhantomData,
        })
    }
}

impl<P: GlobalPermissionMarker + Send + Sync> FromRequestParts<AppState> for AuthorizedGlobal<P> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let CurrentActor(actor) = CurrentActor::from_request_parts(parts, state).await?;
        if !actor.can_global(P::PERMISSION) {
            return Err(missing_permission(&P::PERMISSION));
        }
        Ok(Self {
            actor,
            _permission: PhantomData,
        })
    }
}

async fn instance_id_from_path(parts: &mut Parts, state: &AppState) -> Result<InstanceId, ApiError> {
    let params = RawPathParams::from_request_parts(parts, state)
        .await
        .map_err(|error| ApiError::internal(&format!("route has no path parameters: {error}")))?;
    let raw = params
        .iter()
        .find_map(|(name, value)| (name == "instance_id").then_some(value))
        .ok_or_else(|| ApiError::internal(&"route is missing the {instance_id} path parameter"))?;
    raw.parse()
        .map_err(|_| ApiError::new(StatusCode::NOT_FOUND, "instance_not_found", "No such instance."))
}

fn missing_permission(permission: &impl serde::Serialize) -> ApiError {
    let name = serde_json::to_value(permission)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    ApiError::forbidden(
        "missing_permission",
        format!("You need the \"{name}\" permission for this."),
    )
}

/// Marker types for every permission, for use as `Authorized<perm::X>` / `AuthorizedGlobal<perm::X>`.
pub mod perm {
    use super::{GlobalPermission, GlobalPermissionMarker, InstancePermission, Permission};

    macro_rules! markers {
        ($trait:ident, $enum:ident: $($name:ident),+ $(,)?) => {$(
            #[doc = concat!("Requires `", stringify!($enum), "::", stringify!($name), "`.")]
            #[derive(Debug)]
            pub struct $name;
            impl $trait for $name {
                const PERMISSION: $enum = $enum::$name;
            }
        )+};
    }

    markers!(InstancePermission, Permission:
        InstanceView, ConsoleRead, LogsRead, PlayersManage, ConsoleWrite, Lifecycle, BackupsCreate,
        SchedulesManage, FilesRead, FilesWrite, PropertiesWrite, ContentManage, BackupsRestore,
        InstanceSettings, InstanceUpdate,
    );

    markers!(GlobalPermissionMarker, GlobalPermission:
        InstanceCreate, InstanceDelete, UsersManage, SettingsManage, CustomRuntime, SandboxExceptions,
    );
}
