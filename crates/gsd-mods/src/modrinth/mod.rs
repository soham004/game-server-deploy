//! The Modrinth API (v2): search, projects, versions, and identifying files by hash. No key is
//! needed. Modrinth allows 300 requests a minute; a rate-limited request waits for the reset
//! Modrinth announces and is tried once more.

mod wire;

use std::collections::HashMap;

use serde_json::json;

use crate::{
    error::SourceError,
    http::{segment, send_json},
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort},
    target::{ContentKind, Target},
};

pub const DEFAULT_API: &str = "https://api.modrinth.com/v2";

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

    /// Where the API is.
    pub(crate) fn api(&self) -> &str {
        &self.api
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
        self.run_search(&facets, query).await
    }

    /// Modpacks that run on a server, for `mc_version` if one is given.
    pub async fn search_packs(
        &self,
        query: &SearchQuery,
        mc_version: Option<&str>,
    ) -> Result<SearchPage, SourceError> {
        let mut facets = vec![
            json!(["project_type:modpack"]),
            json!(["server_side:required", "server_side:optional"]),
        ];
        if let Some(version) = mc_version {
            facets.push(json!([format!("versions:{version}")]));
        }
        self.run_search(&json!(facets), query).await
    }

    async fn run_search(
        &self,
        facets: &serde_json::Value,
        query: &SearchQuery,
    ) -> Result<SearchPage, SourceError> {
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
        let response: wire::SearchResponse = send_json(request).await?;
        Ok(response.into())
    }

    /// Every version of a modpack, newest first. A pack decides its own loader and Minecraft
    /// version, so there is nothing to narrow these to.
    pub async fn pack_versions(&self, project_id: &str) -> Result<Vec<ContentVersion>, SourceError> {
        let request = self
            .client
            .get(format!("{}/project/{}/version", self.api, segment(project_id)?));
        let response: Vec<wire::VersionResponse> = send_json(request).await?;
        Ok(response
            .into_iter()
            .filter_map(wire::VersionResponse::into_version)
            .collect())
    }

    pub async fn project(&self, id: &str) -> Result<Project, SourceError> {
        let request = self.client.get(format!("{}/project/{}", self.api, segment(id)?));
        let response: wire::ProjectResponse = send_json(request).await?;
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
        let response: Vec<wire::ProjectResponse> = send_json(request).await?;
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
        let response: Vec<wire::VersionResponse> = send_json(request).await?;
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
        let response: HashMap<String, wire::VersionResponse> = send_json(request).await?;
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
        let response: HashMap<String, wire::VersionResponse> = send_json(request).await?;
        Ok(versions_by_hash(response))
    }
}

fn versions_by_hash(response: HashMap<String, wire::VersionResponse>) -> HashMap<String, ContentVersion> {
    response
        .into_iter()
        .filter_map(|(hash, version)| version.into_version().map(|version| (hash, version)))
        .collect()
}

#[cfg(test)]
mod tests;
