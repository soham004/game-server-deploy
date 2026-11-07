//! Security headers on every response.
//!
//! The Content-Security-Policy forbids inline scripts and styles, which also keeps the
//! frontend honest: all JS lives in modules under `web/js`, all CSS under `web/css`.

use axum::{
    extract::Request,
    http::{HeaderName, HeaderValue, header},
    middleware::Next,
    response::Response,
};

const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; style-src 'self'; \
     img-src 'self' data: https:; font-src 'self'; connect-src 'self'; object-src 'none'; \
     base-uri 'none'; frame-ancestors 'none'; form-action 'self'";

const HEADERS: [(HeaderName, &str); 5] = [
    (header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY),
    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    (header::X_FRAME_OPTIONS, "DENY"),
    (header::REFERRER_POLICY, "same-origin"),
    (
        HeaderName::from_static("permissions-policy"),
        "camera=(), microphone=(), geolocation=()",
    ),
];

pub async fn add_security_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    for (name, value) in HEADERS {
        headers.entry(name).or_insert(HeaderValue::from_static(value));
    }
    response
}
