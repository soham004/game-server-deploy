//! HTTP GET with caching for catalog data.
//!
//! Responses are kept in memory for a few minutes and on disk indefinitely. When an upstream is
//! down or the host is offline, the last disk copy is served (stale but usable), so the version
//! picker and installs of already-seen versions keep working.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::sync::Mutex;

use super::CatalogError;

/// How long a response is reused before asking upstream again.
const MEMORY_TTL: Duration = Duration::from_secs(10 * 60);

/// Cached GETs. Cheap to clone.
#[derive(Debug, Clone)]
pub struct Fetcher {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    client: reqwest::Client,
    disk_cache: PathBuf,
    memory: Mutex<HashMap<String, (Instant, Value)>>,
}

impl Fetcher {
    pub fn new(client: reqwest::Client, disk_cache: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                client,
                disk_cache,
                memory: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.inner.client
    }

    /// GET and decode JSON.
    pub async fn json<T: DeserializeOwned>(&self, url: &str) -> Result<T, CatalogError> {
        let value = self.cached(url, Body::Json).await?;
        serde_json::from_value(value).map_err(|error| malformed(url, error))
    }

    /// GET as text (Maven XML metadata, `.sha1` files).
    pub async fn text(&self, url: &str) -> Result<String, CatalogError> {
        match self.cached(url, Body::Text).await? {
            Value::String(text) => Ok(text),
            _ => Err(malformed(url, "expected text")),
        }
    }

    async fn cached(&self, url: &str, body: Body) -> Result<Value, CatalogError> {
        if let Some((fetched, value)) = self.inner.memory.lock().await.get(url)
            && fetched.elapsed() < MEMORY_TTL
        {
            return Ok(value.clone());
        }
        match self.download(url, body).await {
            Ok(value) => {
                self.remember(url, &value).await;
                Ok(value)
            }
            Err(error) => match self.read_disk(url).await {
                Some(stale) => {
                    tracing::warn!(%url, %error, "using cached catalog data");
                    Ok(stale)
                }
                None => Err(error),
            },
        }
    }

    async fn download(&self, url: &str, body: Body) -> Result<Value, CatalogError> {
        let response =
            self.inner
                .client
                .get(url)
                .send()
                .await
                .map_err(|source| CatalogError::Unreachable {
                    url: url.into(),
                    source,
                })?;
        if !response.status().is_success() {
            return Err(CatalogError::Status {
                url: url.into(),
                status: response.status().as_u16(),
            });
        }
        match body {
            Body::Json => response.json().await.map_err(|error| malformed(url, error)),
            Body::Text => Ok(Value::String(
                response.text().await.map_err(|error| malformed(url, error))?,
            )),
        }
    }

    async fn remember(&self, url: &str, value: &Value) {
        self.inner
            .memory
            .lock()
            .await
            .insert(url.to_owned(), (Instant::now(), value.clone()));
        if let Ok(bytes) = serde_json::to_vec(value) {
            let _ = tokio::fs::create_dir_all(&self.inner.disk_cache).await;
            let _ = tokio::fs::write(self.disk_path(url), bytes).await;
        }
    }

    async fn read_disk(&self, url: &str) -> Option<Value> {
        let bytes = tokio::fs::read(self.disk_path(url)).await.ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn disk_path(&self, url: &str) -> PathBuf {
        let name: String = url
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        self.inner.disk_cache.join(format!("{name}.json"))
    }
}

#[derive(Debug, Clone, Copy)]
enum Body {
    Json,
    Text,
}

fn malformed(url: &str, detail: impl ToString) -> CatalogError {
    CatalogError::Malformed {
        url: url.into(),
        detail: detail.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    async fn server() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/data.json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({ "a": 1 })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/file.sha1"))
            .respond_with(ResponseTemplate::new(200).set_body_string("abc123  file.jar\n"))
            .mount(&server)
            .await;
        server
    }

    fn fetcher(dir: &std::path::Path) -> Fetcher {
        Fetcher::new(gsd_core::net::http_client().unwrap(), dir.to_owned())
    }

    #[tokio::test]
    async fn json_and_text_survive_an_upstream_outage() {
        let server = server().await;
        let dir = tempfile::tempdir().unwrap();
        let online = fetcher(dir.path());
        let json: Value = online.json(&format!("{}/data.json", server.uri())).await.unwrap();
        let text = online.text(&format!("{}/file.sha1", server.uri())).await.unwrap();

        server.reset().await;
        let offline = fetcher(dir.path()); // empty memory cache: must use the disk copy
        assert_eq!(
            offline
                .json::<Value>(&format!("{}/data.json", server.uri()))
                .await
                .unwrap(),
            json
        );
        assert_eq!(
            offline
                .text(&format!("{}/file.sha1", server.uri()))
                .await
                .unwrap(),
            text
        );
    }

    #[tokio::test]
    async fn errors_name_the_url() {
        let server = server().await;
        let dir = tempfile::tempdir().unwrap();
        let missing = format!("{}/missing.json", server.uri());
        match fetcher(dir.path()).json::<Value>(&missing).await {
            Err(CatalogError::Status { url, status }) => assert_eq!((url, status), (missing, 404)),
            other => panic!("unexpected {other:?}"),
        }
        let not_json = format!("{}/file.sha1", server.uri());
        assert!(matches!(
            fetcher(dir.path()).json::<Value>(&not_json).await,
            Err(CatalogError::Malformed { .. })
        ));
    }
}
