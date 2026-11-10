//! Mojang's public profile API: a name's account (for adding a player to a stopped server), and an
//! account's skin (for its head). No key is needed. Answers are cached, so a page of heads or a list of
//! names doesn't ask again, and heads are kept on disk, so they show when Mojang can't be reached.
//!
//! The skin's address comes from Mojang's answer; only one on the textures host is fetched, and by its
//! hash alone, so that answer can't send the app anywhere else.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use base64::Engine;
use serde::Deserialize;

use super::{
    head::{self, HeadError},
    names::{is_account_uuid, is_valid_name, normalize_uuid},
};

/// How long a name's answer is kept; a name without an account is asked about again sooner.
const NAME_TTL: Duration = Duration::from_secs(60 * 60);
const UNKNOWN_NAME_TTL: Duration = Duration::from_secs(10 * 60);
/// Heads change when skins do; a day is soon enough.
const HEAD_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const NO_SKIN_TTL: Duration = Duration::from_secs(60 * 60);
/// Entries kept in memory before the oldest are forgotten.
const MAX_CACHED: usize = 2000;
/// Skins are a few kilobytes.
const MAX_SKIN_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone)]
pub struct ProfileEndpoints {
    /// `GET {api}/users/profiles/minecraft/{name}`.
    pub api: String,
    /// `GET {sessions}/session/minecraft/profile/{uuid}`.
    pub sessions: String,
    /// Where skins are, as Mojang's answers name them; only addresses starting with this are used…
    pub textures_named: String,
    /// …and fetched from here, with their hash.
    pub textures: String,
}

impl Default for ProfileEndpoints {
    fn default() -> Self {
        Self {
            api: "https://api.mojang.com".into(),
            sessions: "https://sessionserver.mojang.com".into(),
            textures_named: "http://textures.minecraft.net/texture/".into(),
            textures: "https://textures.minecraft.net/texture/".into(),
        }
    }
}

impl ProfileEndpoints {
    /// Everything on one host (tests: a mock server).
    pub fn all_at(base: &str) -> Self {
        Self {
            api: base.into(),
            sessions: base.into(),
            textures_named: format!("{base}/texture/"),
            textures: format!("{base}/texture/"),
        }
    }
}

/// An account: its UUID (with hyphens) and its name as Mojang spells it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub uuid: String,
    pub name: String,
}

#[derive(Debug, thiserror::Error)]
pub enum MojangError {
    #[error("could not reach Mojang: {0}")]
    Unreachable(String),
    #[error("Mojang answered HTTP {0}")]
    Status(u16),
    #[error("Mojang's answer wasn't what was expected")]
    Unexpected,
    #[error(transparent)]
    Head(#[from] HeadError),
}

#[derive(Debug, Clone)]
pub struct Mojang {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    client: reqwest::Client,
    endpoints: ProfileEndpoints,
    heads_dir: PathBuf,
    names: Mutex<HashMap<String, (Instant, Option<Profile>)>>,
    no_skin: Mutex<HashMap<String, (Instant, ())>>,
}

#[derive(Deserialize)]
struct NameAnswer {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct ProfileAnswer {
    #[serde(default)]
    properties: Vec<Property>,
}

#[derive(Deserialize)]
struct Property {
    name: String,
    value: String,
}

impl Mojang {
    pub fn new(client: reqwest::Client, endpoints: ProfileEndpoints, heads_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                endpoints,
                heads_dir,
                names: Mutex::new(HashMap::new()),
                no_skin: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// The account with this name, if there is one.
    pub async fn lookup(&self, name: &str) -> Result<Option<Profile>, MojangError> {
        // It goes in the address: nothing but a name an account could have.
        if !is_valid_name(name) {
            return Ok(None);
        }
        let key = name.to_ascii_lowercase();
        if let Some(cached) = cached(&self.inner.names, &key, |found: &Option<Profile>| {
            if found.is_some() {
                NAME_TTL
            } else {
                UNKNOWN_NAME_TTL
            }
        }) {
            return Ok(cached);
        }
        let url = format!("{}/users/profiles/minecraft/{name}", self.inner.endpoints.api);
        let response = self.get(&url).await?;
        let found = match response.status().as_u16() {
            200 => {
                let answer: NameAnswer = response.json().await.map_err(|_| MojangError::Unexpected)?;
                let uuid = normalize_uuid(&answer.id).ok_or(MojangError::Unexpected)?;
                Some(Profile {
                    uuid,
                    name: answer.name,
                })
            }
            204 | 404 => None,
            status => return Err(MojangError::Status(status)),
        };
        remember(&self.inner.names, key, found.clone());
        Ok(found)
    }

    /// The head of an account, as a PNG; `None` when it has no skin of its own (the game then shows one of
    /// its default skins), or isn't an account at all (offline-mode players). A head fetched before is
    /// used when Mojang can't be reached.
    pub async fn head(&self, uuid: &str) -> Result<Option<Vec<u8>>, MojangError> {
        let Some(uuid) = normalize_uuid(uuid).filter(|uuid| is_account_uuid(uuid)) else {
            return Ok(None);
        };
        let file = self.inner.heads_dir.join(format!("{uuid}.png"));
        let saved = tokio::fs::read(&file).await.ok();
        let fresh = tokio::fs::metadata(&file)
            .await
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age < HEAD_TTL);
        if fresh && saved.is_some() {
            return Ok(saved);
        }
        if cached(&self.inner.no_skin, &uuid, |()| NO_SKIN_TTL).is_some() {
            return Ok(None);
        }
        match self.fetch_head(&uuid).await {
            Ok(Some(head)) => {
                if let Err(error) = save(&file, &head).await {
                    tracing::warn!(%error, "could not keep a player's head");
                }
                Ok(Some(head))
            }
            Ok(None) => {
                remember(&self.inner.no_skin, uuid, ());
                Ok(None)
            }
            Err(error) => saved.map_or(Err(error), |old| Ok(Some(old))),
        }
    }

    async fn fetch_head(&self, uuid: &str) -> Result<Option<Vec<u8>>, MojangError> {
        let endpoints = &self.inner.endpoints;
        let url = format!(
            "{}/session/minecraft/profile/{}",
            endpoints.sessions,
            uuid.replace('-', "")
        );
        let response = self.get(&url).await?;
        match response.status().as_u16() {
            200 => {}
            204 | 404 => return Ok(None),
            status => return Err(MojangError::Status(status)),
        }
        let answer: ProfileAnswer = response.json().await.map_err(|_| MojangError::Unexpected)?;
        let Some(hash) = skin_hash(&answer, &endpoints.textures_named) else {
            return Ok(None);
        };
        let response = self.get(&format!("{}{hash}", endpoints.textures)).await?;
        if !response.status().is_success() {
            return Err(MojangError::Status(response.status().as_u16()));
        }
        let skin = response
            .bytes()
            .await
            .map_err(|error| MojangError::Unreachable(error.to_string()))?;
        if skin.len() > MAX_SKIN_BYTES {
            return Err(MojangError::Unexpected);
        }
        Ok(Some(head::render(&skin)?))
    }

    async fn get(&self, url: &str) -> Result<reqwest::Response, MojangError> {
        self.inner
            .client
            .get(url)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|error| MojangError::Unreachable(error.to_string()))
    }
}

/// The skin's hash from a profile's `textures` property, when its address is on the textures host.
fn skin_hash(answer: &ProfileAnswer, textures_named: &str) -> Option<String> {
    let textures = answer
        .properties
        .iter()
        .find(|property| property.name == "textures")?;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&textures.value)
        .ok()?;
    let value: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    let url = value.pointer("/textures/SKIN/url")?.as_str()?;
    let hash = url.strip_prefix(textures_named)?;
    (!hash.is_empty() && hash.len() <= 128 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hash.to_owned())
}

fn cached<T: Clone>(
    map: &Mutex<HashMap<String, (Instant, T)>>,
    key: &str,
    ttl: impl Fn(&T) -> Duration,
) -> Option<T> {
    let map = map.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    map.get(key)
        .filter(|(at, value)| at.elapsed() < ttl(value))
        .map(|(_, value)| value.clone())
}

fn remember<T>(map: &Mutex<HashMap<String, (Instant, T)>>, key: String, value: T) {
    let mut map = map.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if map.len() >= MAX_CACHED {
        map.clear();
    }
    map.insert(key, (Instant::now(), value));
}

/// Written next to its place and renamed, so a head is never half there.
async fn save(file: &std::path::Path, head: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = file.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let partial = file.with_extension("png.partial");
    tokio::fs::write(&partial, head).await?;
    tokio::fs::rename(&partial, file).await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;
    use crate::test_support::test_skin;

    const NOTCH: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";

    fn textures(url: &str) -> String {
        let value = json!({ "textures": { "SKIN": { "url": url } } });
        base64::engine::general_purpose::STANDARD.encode(value.to_string())
    }

    async fn mojang(server: &MockServer, heads: &std::path::Path) -> Mojang {
        Mojang::new(
            gsd_core::net::http_client().unwrap(),
            ProfileEndpoints::all_at(&server.uri()),
            heads.to_owned(),
        )
    }

    #[tokio::test]
    async fn names_are_looked_up_once() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/profiles/minecraft/notch"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": NOTCH.replace('-', ""), "name": "Notch" })),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/users/profiles/minecraft/nobody"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mojang = mojang(&server, dir.path()).await;
        let found = mojang.lookup("notch").await.unwrap().unwrap();
        assert_eq!(
            found,
            Profile {
                uuid: NOTCH.into(),
                name: "Notch".into()
            }
        );
        assert_eq!(
            mojang.lookup("NOTCH").await.unwrap().unwrap().name,
            "Notch",
            "cached, any case"
        );
        assert_eq!(mojang.lookup("nobody").await.unwrap(), None);
    }

    #[tokio::test]
    async fn heads_are_cut_from_skins_and_kept_for_when_mojang_is_down() {
        let server = MockServer::start().await;
        let skin_url = format!("{}/texture/abc123", server.uri());
        Mock::given(method("GET"))
            .and(path(format!("/session/minecraft/profile/{}", NOTCH.replace('-', ""))))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "id": "x", "name": "Notch", "properties": [{ "name": "textures", "value": textures(&skin_url) }] })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/texture/abc123"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(test_skin(64)))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mojang = mojang(&server, dir.path()).await;
        let head = mojang.head(NOTCH).await.unwrap().unwrap();
        assert!(head.starts_with(b"\x89PNG"));
        assert!(dir.path().join(format!("{NOTCH}.png")).exists(), "kept on disk");

        // Mojang gone, the saved head old: the old one is better than none.
        drop(server);
        let file = dir.path().join(format!("{NOTCH}.png"));
        let old = std::fs::File::options().write(true).open(&file).unwrap();
        old.set_modified(SystemTime::now() - HEAD_TTL * 2).unwrap();
        assert_eq!(mojang.head(NOTCH).await.unwrap().unwrap(), head);
    }

    #[tokio::test]
    async fn no_skin_offline_players_and_foreign_addresses_have_no_head() {
        let server = MockServer::start().await;
        let elsewhere = textures("http://169.254.169.254/texture/abc");
        Mock::given(method("GET"))
            .and(path(format!(
                "/session/minecraft/profile/{}",
                NOTCH.replace('-', "")
            )))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "properties": [{ "name": "textures", "value": elsewhere }] })),
            )
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let mojang = mojang(&server, dir.path()).await;
        assert_eq!(
            mojang.head(NOTCH).await.unwrap(),
            None,
            "a skin somewhere else isn't fetched"
        );
        assert_eq!(
            mojang.head(NOTCH).await.unwrap(),
            None,
            "and isn't asked about again soon"
        );
        let offline = crate::players::names::offline_uuid("Steve");
        assert_eq!(
            mojang.head(&offline).await.unwrap(),
            None,
            "offline players have no account"
        );
        assert_eq!(mojang.head("../etc/passwd").await.unwrap(), None);
    }
}
