//! The `CurseForge` API: search, projects, files, and recognising files by their fingerprint.
//!
//! Every request needs the user's API key (stored in the app's settings). The key goes only to
//! `CurseForge`'s API, in a header marked sensitive, and is never logged, returned or sent along with
//! downloads. Authors can opt out of other apps downloading their files; such a file has no
//! download address and is listed for the user to fetch by hand (see `file.manual_url`).

mod wire;

use std::{
    collections::HashMap,
    fmt,
    sync::{Arc, PoisonError, RwLock},
};

use reqwest::header::HeaderValue;
use serde_json::json;

use crate::{
    error::SourceError,
    http::send_json,
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort},
    target::{ContentKind, Target},
};

pub const DEFAULT_API: &str = "https://api.curseforge.com/v1";

const KEY_HEADER: &str = "x-api-key";
/// Minecraft on `CurseForge`.
const GAME_ID: u32 = 432;
const CLASS_MODS: u32 = 6;
const CLASS_BUKKIT_PLUGINS: u32 = 5;
/// The most files one request returns.
const FILES_PAGE: &str = "50";
/// `CurseForge` answers nothing beyond this result (`index + pageSize`).
const MAX_RESULTS: u32 = 10_000;

/// A key this can't send as a header.
#[derive(Debug, thiserror::Error)]
#[error("that doesn't look like an API key")]
pub struct InvalidKey;

#[derive(Clone)]
pub struct CurseForge {
    client: reqwest::Client,
    api: String,
    key: Arc<RwLock<Option<HeaderValue>>>,
}

/// Written out so the key can't reach a log through `{:?}`.
impl fmt::Debug for CurseForge {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CurseForge")
            .field("api", &self.api)
            .field("has_key", &self.has_key())
            .finish_non_exhaustive()
    }
}

impl CurseForge {
    pub fn new(client: reqwest::Client, api: impl Into<String>) -> Self {
        Self {
            client,
            api: api.into(),
            key: Arc::default(),
        }
    }

    /// Uses `key` from now on (none: `CurseForge` is off).
    pub fn set_key(&self, key: Option<&str>) -> Result<(), InvalidKey> {
        let value = key.map(key_header).transpose()?;
        *self.key.write().unwrap_or_else(PoisonError::into_inner) = value;
        Ok(())
    }

    pub fn has_key(&self) -> bool {
        self.key.read().unwrap_or_else(PoisonError::into_inner).is_some()
    }

    /// Asks `CurseForge` whether it accepts `key`, without starting to use it.
    pub async fn check_key(&self, key: &str) -> Result<(), SourceError> {
        let header = key_header(key).map_err(|_| SourceError::KeyRefused)?;
        let request = self
            .client
            .get(format!("{}/games/{GAME_ID}", self.api))
            .header(KEY_HEADER, header);
        let _: serde_json::Value = send_json(request).await?;
        Ok(())
    }

    fn key(&self) -> Result<HeaderValue, SourceError> {
        self.key
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(SourceError::NoKey)
    }

    fn get(&self, path: &str) -> Result<reqwest::RequestBuilder, SourceError> {
        Ok(self
            .client
            .get(format!("{}{path}", self.api))
            .header(KEY_HEADER, self.key()?))
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::RequestBuilder, SourceError> {
        Ok(self
            .client
            .post(format!("{}{path}", self.api))
            .header(KEY_HEADER, self.key()?)
            .json(body))
    }

    /// Projects that fit `target`. `CurseForge` can't filter by side, so client-only mods show too.
    pub async fn search(&self, target: &Target, query: &SearchQuery) -> Result<SearchPage, SourceError> {
        let class = match target.kind {
            ContentKind::Mods => CLASS_MODS,
            ContentKind::Plugins => CLASS_BUKKIT_PLUGINS,
        };
        let sort = match query.sort {
            // With text and no sort field, CurseForge orders by how well it matches.
            SearchSort::Relevance if !query.text.is_empty() => None,
            SearchSort::Relevance => Some("2"), // popularity
            SearchSort::Downloads => Some("6"),
            SearchSort::Updated => Some("3"),
            SearchSort::Newest => Some("11"),
        };
        let page_size = query.limit.min(50);
        let index = query.offset.min(MAX_RESULTS - page_size);
        let mut parameters: Vec<(&str, String)> = vec![
            ("gameId", GAME_ID.to_string()),
            ("classId", class.to_string()),
            ("gameVersion", target.mc_version.clone()),
            ("searchFilter", query.text.clone()),
            ("sortOrder", "desc".into()),
            ("index", index.to_string()),
            ("pageSize", page_size.to_string()),
        ];
        match loader_ids(target).as_slice() {
            [] => {}
            [one] => parameters.push(("modLoaderType", one.to_string())),
            several => parameters.push(("modLoaderTypes", json!(several).to_string())),
        }
        if let Some(sort) = sort {
            parameters.push(("sortField", sort.into()));
        }
        let request = self.get("/mods/search")?.query(&parameters);
        let response: wire::SearchResponse = send_json(request).await?;
        Ok(wire::SearchPageParts::from(response).at(query.offset))
    }

    pub async fn project(&self, id: &str) -> Result<Project, SourceError> {
        let id = number(id)?;
        let response: wire::Data<wire::ModResponse> = send_json(self.get(&format!("/mods/{id}"))?).await?;
        Ok(response.data.into_project())
    }

    /// Several projects in one request (unknown ids are left out).
    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>, SourceError> {
        let ids: Vec<u64> = ids.iter().filter_map(|id| number(id).ok()).collect();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let request = self.post("/mods", &json!({ "modIds": ids, "filterPcOnly": false }))?;
        let response: wire::Data<Vec<wire::ModResponse>> = send_json(request).await?;
        Ok(response
            .data
            .into_iter()
            .map(wire::ModResponse::into_project)
            .collect())
    }

    /// A project's files that fit `target`, newest first.
    pub async fn versions(
        &self,
        project_id: &str,
        target: &Target,
    ) -> Result<Vec<ContentVersion>, SourceError> {
        let id = number(project_id)?;
        let mut parameters = vec![
            ("gameVersion", target.mc_version.clone()),
            ("pageSize", FILES_PAGE.to_owned()),
        ];
        // The files endpoint takes one loader; with several (Quilt), they are told apart below.
        if let [one] = loader_ids(target).as_slice() {
            parameters.push(("modLoaderType", one.to_string()));
        }
        let request = self.get(&format!("/mods/{id}/files"))?.query(&parameters);
        let response: wire::Data<Vec<wire::FileResponse>> = send_json(request).await?;
        let files: Vec<wire::FileResponse> = response
            .data
            .into_iter()
            .filter(wire::FileResponse::is_offered)
            .collect();
        // The address for fetching by hand: only worth a request when something needs it.
        let page = if files.iter().any(wire::FileResponse::is_blocked) {
            Some(self.project(project_id).await?.page_url)
        } else {
            None
        };
        let mut versions: Vec<ContentVersion> = files
            .into_iter()
            .map(|file| file.into_version(page.as_deref()))
            .filter(|version| target.accepts(&version.loaders, &version.game_versions))
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.published));
        Ok(versions)
    }

    /// Files by their ids, as a `CurseForge` modpack names them (unknown ones are left out).
    pub async fn files(&self, ids: &[u64]) -> Result<Vec<ContentVersion>, SourceError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let request = self.post("/mods/files", &json!({ "fileIds": ids }))?;
        let response: wire::Data<Vec<wire::FileResponse>> = send_json(request).await?;
        Ok(response
            .data
            .into_iter()
            .map(|file| file.into_version(None))
            .collect())
    }

    /// The versions files with these fingerprints belong to (unknown ones are left out).
    pub async fn identify(&self, fingerprints: &[u32]) -> Result<HashMap<u32, ContentVersion>, SourceError> {
        if fingerprints.is_empty() {
            return Ok(HashMap::new());
        }
        let request = self.post(
            &format!("/fingerprints/{GAME_ID}"),
            &json!({ "fingerprints": fingerprints }),
        )?;
        let response: wire::Data<wire::FingerprintMatches> = send_json(request).await?;
        Ok(response
            .data
            .exact_matches
            .into_iter()
            .filter_map(|found| {
                let fingerprint = u32::try_from(found.file.file_fingerprint).ok()?;
                Some((fingerprint, found.file.into_version(None)))
            })
            .collect())
    }
}

/// The key as a header value, marked so that it never shows in a request's debug output.
fn key_header(key: &str) -> Result<HeaderValue, InvalidKey> {
    let mut value = HeaderValue::from_str(key).map_err(|_| InvalidKey)?;
    value.set_sensitive(true);
    Ok(value)
}

/// `CurseForge` ids are numbers; anything else can't name a project there.
fn number(id: &str) -> Result<u64, SourceError> {
    id.parse().map_err(|_| SourceError::NotFound)
}

/// `CurseForge`'s numbers for the loaders `target` accepts (none for plugins).
fn loader_ids(target: &Target) -> Vec<u32> {
    target
        .loaders
        .iter()
        .filter_map(|loader| match *loader {
            "forge" => Some(1),
            "fabric" => Some(4),
            "quilt" => Some(5),
            "neoforge" => Some(6),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
