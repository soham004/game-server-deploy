//! What every content source's client does with HTTP: send, wait out a rate limit once, map the
//! status to a [`SourceError`], decode the JSON. And checking ids before they go into a URL.

use std::time::Duration;

use reqwest::{RequestBuilder, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::SourceError;

/// The longest a rate-limited request waits before its one retry.
const MAX_RATE_LIMIT_WAIT: Duration = Duration::from_secs(60);

/// Sends, waiting out a rate limit once, and decodes the JSON body.
pub(crate) async fn send_json<T: DeserializeOwned>(request: RequestBuilder) -> Result<T, SourceError> {
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
        // CurseForge answers this way to a wrong key.
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(SourceError::KeyRefused),
        // Modrinth retires old API versions this way.
        StatusCode::GONE => Err(SourceError::ApiRetired),
        StatusCode::TOO_MANY_REQUESTS => Err(SourceError::RateLimited),
        status => Err(SourceError::Status {
            url,
            status: status.as_u16(),
        }),
    }
}

/// How long the source says to wait (Modrinth's `X-Ratelimit-Reset` or the standard
/// `Retry-After`, in seconds), capped.
fn rate_limit_wait(response: &reqwest::Response) -> Duration {
    ["x-ratelimit-reset", "retry-after"]
        .into_iter()
        .find_map(|name| response.headers().get(name)?.to_str().ok()?.parse::<u64>().ok())
        .map_or(Duration::from_secs(1), Duration::from_secs)
        .min(MAX_RATE_LIMIT_WAIT)
}

/// An id or slug as one URL path segment. Sources use base62, numbers or kebab-case; anything
/// else can't name a project there, so it is "not found" rather than sent.
pub(crate) fn segment(id: &str) -> Result<&str, SourceError> {
    let valid = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if valid { Ok(id) } else { Err(SourceError::NotFound) }
}
