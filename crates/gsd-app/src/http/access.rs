//! Deny by default: every request needs a valid session, except the few a signed-out visitor
//! needs to sign in, and, before the first account exists, to set the app up.
//!
//! - Always: the sign-in page and the files it loads, `POST /api/v1/auth/login` and
//!   `POST /api/v1/auth/logout`, and `GET /api/v1/health` (container and load-balancer health
//!   checks; it says only "ok").
//! - Only while no account exists: the setup page and its files, `GET /api/v1/setup/status` and
//!   `POST /api/v1/setup`.
//!
//! Anything else without a valid session is refused here, before routing, so an outsider can't
//! tell which routes exist or which methods they take: API calls get `401 unauthenticated`, page
//! loads are sent to the sign-in page (or the setup page), and any other file gets `401`.
//!
//! Handlers still check who may do what with their extractors (`authorize.rs`). This layer means a
//! handler that forgets one is unreachable from outside instead of public. The session it finds is
//! passed on in the request's extensions, so the extractors don't look it up a second time.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use gsd_core::auth::{Actor, AuthError};

use crate::{
    http::{error::ApiError, session::session_token},
    state::AppState,
};

/// The sign-in page and every file it loads (checked against the page in the tests).
pub const SIGN_IN_FILES: &[&str] = &[
    "login.html",
    "img/icon.svg",
    "css/tokens.css",
    "css/base.css",
    "css/components/button.css",
    "css/components/form.css",
    "css/components/auth.css",
    "js/pages/login.js",
    "js/core/api.js",
    "js/core/dom.js",
    "js/core/forms.js",
];

/// The first-run setup page and every file it loads (checked against the page in the tests).
pub const SETUP_FILES: &[&str] = &[
    "setup.html",
    "img/icon.svg",
    "css/tokens.css",
    "css/base.css",
    "css/components/button.css",
    "css/components/form.css",
    "css/components/auth.css",
    "js/pages/setup.js",
    "js/core/api.js",
    "js/core/dom.js",
    "js/core/forms.js",
];

const SIGN_IN_PAGE: &str = "/login.html";
const SETUP_PAGE: &str = "/setup.html";

/// The session behind a request, found by [`require_session`].
#[derive(Debug, Clone)]
pub struct Authenticated(pub Actor);

/// What a visitor without a valid session gets.
#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    /// A page load: go to this page instead.
    Redirect(String),
    Refuse,
}

/// The layer itself (see the module docs).
pub async fn require_session(State(state): State<AppState>, mut request: Request, next: Next) -> Response {
    if let Some(token) = session_token(request.headers()) {
        match state.auth.authenticate(&token).await {
            Ok(actor) => {
                request.extensions_mut().insert(Authenticated(actor));
                return next.run(request).await;
            }
            // Expired, signed out or never valid: the same as no cookie at all.
            Err(AuthError::SessionInvalid) => {}
            Err(error) => return ApiError::from(error).into_response(),
        }
    }

    let setup_required = match state.auth.is_setup_required().await {
        Ok(required) => required,
        Err(error) => return ApiError::from(error).into_response(),
    };
    let uri = request.uri();
    match verdict(request.method(), uri.path(), uri.query(), setup_required) {
        Verdict::Allow => next.run(request).await,
        Verdict::Redirect(location) => see_other(&location),
        Verdict::Refuse => ApiError::unauthenticated().into_response(),
    }
}

/// The rules for a visitor without a session (pure, so every case is tested directly).
fn verdict(method: &Method, path: &str, query: Option<&str>, setup_required: bool) -> Verdict {
    if path == "/api" || path.starts_with("/api/") {
        let open = match (method.as_str(), path) {
            ("POST", "/api/v1/auth/login" | "/api/v1/auth/logout") | ("GET" | "HEAD", "/api/v1/health") => {
                true
            }
            ("GET" | "HEAD", "/api/v1/setup/status") | ("POST", "/api/v1/setup") => setup_required,
            _ => false,
        };
        return if open { Verdict::Allow } else { Verdict::Refuse };
    }
    if method != Method::GET && method != Method::HEAD {
        return Verdict::Refuse;
    }

    let file = path.strip_prefix('/').unwrap_or(path);
    // While there is no account yet, there is nobody to sign in: setup comes first.
    if setup_required {
        return if SETUP_FILES.contains(&file) {
            Verdict::Allow
        } else if is_page(file) {
            Verdict::Redirect(SETUP_PAGE.to_owned())
        } else {
            Verdict::Refuse
        };
    }
    if SIGN_IN_FILES.contains(&file) {
        Verdict::Allow
    } else if path == SETUP_PAGE {
        // Set up already: someone who opens the setup page wants to sign in.
        Verdict::Redirect(SIGN_IN_PAGE.to_owned())
    } else if is_page(file) {
        Verdict::Redirect(sign_in_then(path, query))
    } else {
        Verdict::Refuse
    }
}

/// A page someone opens in the browser (the app shell, an HTML file or a client-side route),
/// as opposed to a file a page loads.
fn is_page(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or_default();
    let extension = std::path::Path::new(name).extension();
    name.is_empty() || extension.is_none_or(|extension| extension.eq_ignore_ascii_case("html"))
}

/// The sign-in page, returning to `path` afterwards (the page checks it stays on this site).
fn sign_in_then(path: &str, query: Option<&str>) -> String {
    let back = match query {
        Some(query) => format!("{path}?{query}"),
        None => path.to_owned(),
    };
    if back == "/" {
        return SIGN_IN_PAGE.to_owned();
    }
    format!("{SIGN_IN_PAGE}?next={}", encode_component(&back))
}

/// Percent-encodes everything but unreserved characters (RFC 3986), for a query value.
fn encode_component(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// `303 See Other`, never cached: the answer changes once the visitor signs in.
fn see_other(location: &str) -> Response {
    let mut response = StatusCode::SEE_OTHER.into_response();
    let headers = response.headers_mut();
    // WHY: the location is a fixed page or one with a percent-encoded query: always valid.
    if let Ok(value) = HeaderValue::from_str(location) {
        headers.insert(header::LOCATION, value);
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[cfg(test)]
mod tests;
