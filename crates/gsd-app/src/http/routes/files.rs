//! The file manager: browse, edit, upload, download and reorganise an instance's server files.
//!
//! Every path is parsed into a `SafeRelPath` here and every operation goes through
//! `gsd_core::files::ServerFiles`, which keeps it inside the server folder. Downloads are always
//! sent as attachments with a generic type, so a hostile HTML file can't run on this origin.

use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, Query, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use gsd_core::{
    audit::{AuditEntry, actions},
    files::{Entry, ExtractLimits, ExtractSummary, SafeRelPath, ServerFiles, TextFile},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    http::{
        authorize::{Authorized, perm},
        dto::ApiJson,
        error::ApiError,
    },
    state::AppState,
};

/// Largest upload accepted (worlds and modpacks can be big).
const MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/instances/{instance_id}/files", get(list))
        .route(
            "/instances/{instance_id}/files/text",
            get(read_text).put(write_text),
        )
        .route("/instances/{instance_id}/files/download", get(download))
        .route(
            "/instances/{instance_id}/files/upload",
            put(upload).layer(DefaultBodyLimit::disable()),
        )
        .route("/instances/{instance_id}/files/folder", post(create_folder))
        .route("/instances/{instance_id}/files/rename", post(rename))
        .route("/instances/{instance_id}/files/copy", post(copy))
        .route("/instances/{instance_id}/files/delete", post(delete))
        .route("/instances/{instance_id}/files/extract", post(extract))
        .route("/instances/{instance_id}/files/compress", post(compress))
}

#[derive(Debug, Deserialize)]
struct PathQuery {
    #[serde(default)]
    path: String,
}

#[derive(Debug, Serialize)]
struct Listing {
    path: SafeRelPath,
    entries: Vec<Entry>,
    /// The server is running: edits to files it uses take effect after a restart (or get
    /// overwritten by it).
    running: bool,
}

async fn list(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesRead>,
    Query(query): Query<PathQuery>,
) -> Result<Json<Listing>, ApiError> {
    let path = parse(&query.path)?;
    let entries = files(&state, &auth).await?.list(path.clone()).await?;
    Ok(Json(Listing {
        path,
        entries,
        running: state.instances.state(auth.instance_id).is_active(),
    }))
}

async fn read_text(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesRead>,
    Query(query): Query<PathQuery>,
) -> Result<Json<TextFile>, ApiError> {
    let path = parse(&query.path)?;
    Ok(Json(files(&state, &auth).await?.read_text(path).await?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WriteText {
    path: String,
    content: String,
    /// The revision the editor opened; absent to create a new file.
    #[serde(default)]
    revision: Option<String>,
}

async fn write_text(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<WriteText>,
) -> Result<Json<TextFile>, ApiError> {
    let path = parse(&body.path)?;
    let saved = files(&state, &auth)
        .await?
        .write_text(path.clone(), body.content, body.revision)
        .await?;
    audit(&state, &auth, "save", &[&path]).await;
    Ok(Json(saved))
}

/// A file as-is, or a folder as a zip made on the fly.
async fn download(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesRead>,
    Query(query): Query<PathQuery>,
) -> Result<Response, ApiError> {
    let path = parse(&query.path)?;
    let files = files(&state, &auth).await?;
    match files.open_file(path.clone()).await {
        Ok((file, size)) => {
            let name = path.file_name().unwrap_or("download");
            let body = Body::from_stream(tokio_util::io::ReaderStream::new(file));
            let mut response = attachment(body, name);
            response
                .headers_mut()
                .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
            Ok(response)
        }
        Err(gsd_core::files::FilesError::IsADirectory(_)) => {
            let name = format!("{}.zip", path.file_name().unwrap_or("server"));
            Ok(attachment(Body::from_stream(files.zip_folder(path)), &name))
        }
        Err(error) => Err(error.into()),
    }
}

#[derive(Debug, Deserialize)]
struct UploadQuery {
    path: String,
    #[serde(default)]
    overwrite: bool,
}

/// The request body is the file (streamed to disk, never held in memory).
async fn upload(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    Query(query): Query<UploadQuery>,
    body: Body,
) -> Result<(StatusCode, Json<Entry>), ApiError> {
    let path = parse(&query.path)?;
    let entry = files(&state, &auth)
        .await?
        .upload(
            path.clone(),
            body.into_data_stream(),
            query.overwrite,
            MAX_UPLOAD_BYTES,
        )
        .await?;
    audit(&state, &auth, "upload", &[&path]).await;
    Ok((StatusCode::CREATED, Json(entry)))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathBody {
    path: String,
}

async fn create_folder(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<PathBody>,
) -> Result<StatusCode, ApiError> {
    let path = parse(&body.path)?;
    files(&state, &auth).await?.create_dir(path.clone()).await?;
    audit(&state, &auth, "create_folder", &[&path]).await;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FromTo {
    from: String,
    to: String,
}

async fn rename(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<FromTo>,
) -> Result<StatusCode, ApiError> {
    let (from, to) = (parse(&body.from)?, parse(&body.to)?);
    files(&state, &auth)
        .await?
        .rename(from.clone(), to.clone())
        .await?;
    audit(&state, &auth, "rename", &[&from, &to]).await;
    Ok(StatusCode::NO_CONTENT)
}

async fn copy(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<FromTo>,
) -> Result<StatusCode, ApiError> {
    let (from, to) = (parse(&body.from)?, parse(&body.to)?);
    files(&state, &auth).await?.copy(from.clone(), to.clone()).await?;
    audit(&state, &auth, "copy", &[&from, &to]).await;
    Ok(StatusCode::CREATED)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsBody {
    paths: Vec<String>,
}

/// Moves the entries to the instance's trash.
async fn delete(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<PathsBody>,
) -> Result<StatusCode, ApiError> {
    let paths = body
        .paths
        .iter()
        .map(|p| parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    files(&state, &auth).await?.delete(paths.clone()).await?;
    audit(&state, &auth, "delete", &paths.iter().collect::<Vec<_>>()).await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtractBody {
    path: String,
    destination: String,
}

async fn extract(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<ExtractBody>,
) -> Result<Json<ExtractSummary>, ApiError> {
    let (archive, destination) = (parse(&body.path)?, parse(&body.destination)?);
    let summary = files(&state, &auth)
        .await?
        .extract(archive.clone(), destination.clone(), ExtractLimits::default())
        .await?;
    audit(&state, &auth, "extract", &[&archive, &destination]).await;
    Ok(Json(summary))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompressBody {
    paths: Vec<String>,
    destination: String,
}

async fn compress(
    State(state): State<AppState>,
    auth: Authorized<perm::FilesWrite>,
    ApiJson(body): ApiJson<CompressBody>,
) -> Result<StatusCode, ApiError> {
    let sources = body
        .paths
        .iter()
        .map(|p| parse(p))
        .collect::<Result<Vec<_>, _>>()?;
    let destination = parse(&body.destination)?;
    files(&state, &auth)
        .await?
        .compress(sources.clone(), destination.clone())
        .await?;
    let mut paths: Vec<&SafeRelPath> = sources.iter().collect();
    paths.push(&destination);
    audit(&state, &auth, "compress", &paths).await;
    Ok(StatusCode::CREATED)
}

fn parse(raw: &str) -> Result<SafeRelPath, ApiError> {
    SafeRelPath::parse(raw).map_err(|error| gsd_core::files::FilesError::from(error).into())
}

async fn files<P>(state: &AppState, auth: &Authorized<P>) -> Result<ServerFiles, ApiError> {
    let instance = state.instances.get(auth.instance_id).await?;
    Ok(ServerFiles::open(&instance).await?)
}

/// `Content-Disposition: attachment` with the name encoded for any character (RFC 6266/5987).
pub(crate) fn attachment(body: Body, name: &str) -> Response {
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._- ".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded: String = name
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"._-".contains(&b) {
                char::from(b).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect();
    let disposition = format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}");
    let mut response = body.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    response
}

async fn audit<P>(state: &AppState, auth: &Authorized<P>, operation: &str, paths: &[&SafeRelPath]) {
    let entry = AuditEntry::on_instance(auth.actor.id(), auth.instance_id, actions::FILES_CHANGED)
        .with_details(json!({ "operation": operation, "paths": paths }));
    state.audit.record(entry).await;
}
