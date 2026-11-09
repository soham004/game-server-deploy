//! Hangar (`hangar.papermc.io`), the Paper project's plugin repository: search, projects and
//! versions, for servers that run plugins. No key is needed.
//!
//! Hangar names projects and versions by number, publishes a SHA-256 for each file, and lists
//! dependencies by Hangar project (or by a link for ones hosted elsewhere, which can't be
//! installed from here). It has no lookup by file hash, so files added by hand can't be identified
//! through it, and update checks ask once per project.

mod wire;

use crate::{
    error::SourceError,
    http::{segment, send_json},
    model::{ContentVersion, Project, SearchPage, SearchQuery, SearchSort},
    target::{ContentKind, Target},
};

pub const DEFAULT_API: &str = "https://hangar.papermc.io/api/v1";

/// Hangar's platform for Paper and the servers built on it (Purpur, Folia).
const PLATFORM: &str = "PAPER";

/// Versions per request (Hangar's largest page). Projects that publish many snapshots still have a
/// release among their newest 25.
const VERSIONS_PAGE: &str = "25";

#[derive(Debug, Clone)]
pub struct Hangar {
    client: reqwest::Client,
    api: String,
}

impl Hangar {
    pub fn new(client: reqwest::Client, api: impl Into<String>) -> Self {
        Self {
            client,
            api: api.into(),
        }
    }

    /// Whether Hangar has anything for `target`: plugins only.
    pub fn serves(target: &Target) -> bool {
        target.kind == ContentKind::Plugins
    }

    /// Plugins for `target`'s Minecraft version.
    pub async fn search(&self, target: &Target, query: &SearchQuery) -> Result<SearchPage, SourceError> {
        if !Self::serves(target) {
            return Ok(SearchPage {
                hits: Vec::new(),
                offset: query.offset,
                total: 0,
            });
        }
        let sort = match query.sort {
            // Hangar orders by relevance when there is text and no sort; without text, the most
            // downloaded first is the useful "relevance".
            SearchSort::Relevance if !query.text.is_empty() => None,
            SearchSort::Relevance | SearchSort::Downloads => Some("-downloads"),
            SearchSort::Updated => Some("-updated"),
            SearchSort::Newest => Some("-newest"),
        };
        let (offset, limit) = (query.offset.to_string(), query.limit.to_string());
        let mut parameters = vec![
            ("platform", PLATFORM),
            ("version", target.mc_version.as_str()),
            ("offset", offset.as_str()),
            ("limit", limit.as_str()),
        ];
        if !query.text.is_empty() {
            parameters.push(("q", query.text.as_str()));
        }
        if let Some(sort) = sort {
            parameters.push(("sort", sort));
        }
        let request = self
            .client
            .get(format!("{}/projects", self.api))
            .query(&parameters);
        let response: wire::Page<wire::ProjectResponse> = send_json(request).await?;
        Ok(wire::search_page(response, query.offset))
    }

    /// A project by its number or its name.
    pub async fn project(&self, id: &str) -> Result<Project, SourceError> {
        let request = self.client.get(format!("{}/projects/{}", self.api, segment(id)?));
        let response: wire::ProjectResponse = send_json(request).await?;
        Ok(response.into_project())
    }

    /// A project's newest versions that fit `target` and can be downloaded from Hangar, newest
    /// first.
    pub async fn versions(
        &self,
        project_id: &str,
        target: &Target,
    ) -> Result<Vec<ContentVersion>, SourceError> {
        if !Self::serves(target) {
            return Ok(Vec::new());
        }
        let request = self
            .client
            .get(format!("{}/projects/{}/versions", self.api, segment(project_id)?))
            .query(&[
                ("platform", PLATFORM),
                ("platformVersion", target.mc_version.as_str()),
                ("offset", "0"),
                ("limit", VERSIONS_PAGE),
            ]);
        let response: wire::Page<wire::VersionResponse> = send_json(request).await?;
        Ok(response
            .result
            .into_iter()
            .filter_map(wire::VersionResponse::into_version)
            .filter(|version| target.accepts(&version.loaders, &version.game_versions))
            .collect())
    }
}

#[cfg(test)]
mod tests;
