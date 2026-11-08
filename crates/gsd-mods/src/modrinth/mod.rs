//! The Modrinth API (v2): search, projects, versions, and identifying files by hash. No key is
//! needed. Modrinth allows 300 requests a minute; a rate-limited request waits for the reset
//! Modrinth announces and is tried once more.

mod wire;

use std::{collections::HashMap, time::Duration};

use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::json;

use crate::{
    error::SourceError,
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort},
    target::{ContentKind, Target},
};

pub const DEFAULT_API: &str = "https://api.modrinth.com/v2";

/// The longest a rate-limited request waits before its one retry.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone)]
pub struct Modrinth {
    client: reqwest::Client,
    api: String,
}

impl Modrinth {
    pub fn new(client: reqwest::Client, api: impl Into<String>) -> Self {
        Self {
            client,
            api: api.into(),
        }
    }

    /// The HTTP client (downloads use it too).
    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Projects that fit `target`, and run on a server.
    pub async fn search(&self, target: &Target, query: &SearchQuery) -> Result<SearchPage, SourceError> {
        let project_type = match target.kind {
            ContentKind::Mods => "project_type:mod",
            ContentKind::Plugins => "project_type:plugin",
        };
        // Facets: AND between the inner lists, OR within one.
        let facets = json!([
            [project_type],
            target
                .loaders
                .iter()
                .map(|loader| format!("categories:{loader}"))
                .collect::<Vec<_>>(),
            [format!("versions:{}", target.mc_version)],
            ["server_side:required", "server_side:optional"],
        ]);
        let index = match query.sort {
            SearchSort::Relevance => "relevance",
            SearchSort::Downloads => "downloads",
            SearchSort::Updated => "updated",
            SearchSort::Newest => "newest",
        };
        let request = self.client.get(format!("{}/search", self.api)).query(&[
            ("query", query.text.as_str()),
            ("facets", &facets.to_string()),
            ("index", index),
            ("offset", &query.offset.to_string()),
            ("limit", &query.limit.to_string()),
        ]);
        let response: wire::SearchResponse = self.send(request).await?;
        Ok(response.into())
    }

    pub async fn project(&self, id: &str) -> Result<Project, SourceError> {
        let request = self.client.get(format!("{}/project/{}", self.api, segment(id)?));
        let response: wire::ProjectResponse = self.send(request).await?;
        Ok(response.into())
    }

    /// Several projects in one request (unknown ids are left out).
    pub async fn projects(&self, ids: &[String]) -> Result<Vec<Project>, SourceError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        for id in ids {
            segment(id)?;
        }
        let request = self
            .client
            .get(format!("{}/projects", self.api))
            .query(&[("ids", json!(ids).to_string())]);
        let response: Vec<wire::ProjectResponse> = self.send(request).await?;
        Ok(response.into_iter().map(Project::from).collect())
    }

    /// A project's versions that fit `target`, newest first.
    pub async fn versions(
        &self,
        project_id: &str,
        target: &Target,
    ) -> Result<Vec<ContentVersion>, SourceError> {
        let request = self
            .client
            .get(format!("{}/project/{}/version", self.api, segment(project_id)?))
            .query(&[
                ("loaders", json!(target.loaders).to_string()),
                ("game_versions", json!([target.mc_version]).to_string()),
            ]);
        let response: Vec<wire::VersionResponse> = self.send(request).await?;
        Ok(response
            .into_iter()
            .filter_map(wire::VersionResponse::into_version)
            .filter(|version| target.accepts(&version.loaders, &version.game_versions))
            .collect())
    }

    /// The versions files with these SHA-1 hashes belong to (unknown hashes are left out).
    pub async fn identify(&self, sha1s: &[String]) -> Result<HashMap<String, ContentVersion>, SourceError> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let request = self
            .client
            .post(format!("{}/version_files", self.api))
            .json(&json!({ "hashes": sha1s, "algorithm": "sha1" }));
        let response: HashMap<String, wire::VersionResponse> = self.send(request).await?;
        Ok(versions_by_hash(response))
    }

    /// For each SHA-1, the newest version of its project that fits `target`.
    pub async fn latest(
        &self,
        sha1s: &[String],
        target: &Target,
    ) -> Result<HashMap<String, ContentVersion>, SourceError> {
        if sha1s.is_empty() {
            return Ok(HashMap::new());
        }
        let request = self
            .client
            .post(format!("{}/version_files/update", self.api))
            .json(&json!({
                "hashes": sha1s,
                "algorithm": "sha1",
                "loaders": target.loaders,
                "game_versions": [target.mc_version],
            }));
        let response: HashMap<String, wire::VersionResponse> = self.send(request).await?;
        Ok(versions_by_hash(response))
    }

    /// Sends, waiting out a rate limit once, and decodes the JSON body.
    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, SourceError> {
        let retry = request.try_clone();
        let mut response = request.send().await.map_err(SourceError::request)?;
        if response.status() == StatusCode::TOO_MANY_REQUESTS
            && let Some(retry) = retry
        {
            tokio::time::sleep(rate_limit_wait(&response)).await;
            response = retry.send().await.map_err(SourceError::request)?;
        }
        let url = response.url().to_string();
        match response.status() {
            status if status.is_success() => response.json().await.map_err(|error| SourceError::Decode {
                url,
                detail: error.to_string(),
            }),
            StatusCode::NOT_FOUND => Err(SourceError::NotFound),
            StatusCode::GONE => Err(SourceError::ApiRetired),
            StatusCode::TOO_MANY_REQUESTS => Err(SourceError::RateLimited),
            status => Err(SourceError::Status {
                url,
                status: status.as_u16(),
            }),
        }
    }
}

/// How long Modrinth says to wait (`X-Ratelimit-Reset`, seconds), capped.
fn rate_limit_wait(response: &reqwest::Response) -> Duration {
    response
        .headers()
        .get("x-ratelimit-reset")
        .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_secs)
        .min(MAX_RATE_LIMIT_WAIT)
}

fn versions_by_hash(response: HashMap<String, wire::VersionResponse>) -> HashMap<String, ContentVersion> {
    response
        .into_iter()
        .filter_map(|(hash, version)| version.into_version().map(|version| (hash, version)))
        .collect()
}

/// A Modrinth id or slug as one URL path segment (they are base62 or kebab-case).
fn segment(id: &str) -> Result<&str, SourceError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid { Ok(id) } else { Err(SourceError::NotFound) }
}

#[cfg(test)]
mod tests;
