//! Cross-site request forgery protection for the API.
//!
//! Besides the `SameSite=Strict` session cookie, every state-changing request must carry the
//! header `X-CSRF: 1` (a plain HTML form or cross-site `fetch` without CORS cannot set it), and
//! a present `Origin` must match the `Host` the browser addressed.

use axum::{
    extract::Request,
    http::{HeaderMap, header},
    middleware::Next,
    response::Response,
};

use crate::http::error::ApiError;

/// Header the frontend sends on every non-GET request. Brand-neutral on purpose.
pub const CSRF_HEADER: &str = "x-csrf";

pub async fn require_csrf_header(request: Request, next: Next) -> Result<Response, ApiError> {
    if request.method().is_safe() {
        return Ok(next.run(request).await);
    }
    let headers = request.headers();
    if headers.get(CSRF_HEADER).map(axum::http::HeaderValue::as_bytes) != Some(b"1") {
        return Err(ApiError::forbidden("csrf_failed", "Missing X-CSRF header."));
    }
    if !origin_matches_host(headers) {
        return Err(ApiError::forbidden(
            "csrf_failed",
            "Request origin does not match this site.",
        ));
    }
    Ok(next.run(request).await)
}

/// No `Origin` (same-origin GETs, non-browser clients) passes; otherwise its authority must
/// equal the `Host` header.
pub(crate) fn origin_matches_host(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
        return true;
    };
    let host = headers.get(header::HOST).and_then(|value| value.to_str().ok());
    let origin_authority = origin
        .split_once("://")
        .map(|(_, rest)| rest.trim_end_matches('/'));
    matches!((origin_authority, host), (Some(authority), Some(host)) if authority.eq_ignore_ascii_case(host))
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(origin: Option<&'static str>, host: &'static str) -> HeaderMap {
        let mut map = HeaderMap::new();
        map.insert(header::HOST, HeaderValue::from_static(host));
        if let Some(origin) = origin {
            map.insert(header::ORIGIN, HeaderValue::from_static(origin));
        }
        map
    }

    #[test]
    fn same_origin_passes_and_cross_origin_fails() {
        assert!(origin_matches_host(&headers(None, "panel.example.com")));
        assert!(origin_matches_host(&headers(
            Some("https://panel.example.com"),
            "panel.example.com"
        )));
        assert!(origin_matches_host(&headers(
            Some("http://localhost:8080"),
            "localhost:8080"
        )));
        assert!(!origin_matches_host(&headers(
            Some("https://evil.example"),
            "panel.example.com"
        )));
        assert!(!origin_matches_host(&headers(Some("null"), "panel.example.com")));
    }
}
