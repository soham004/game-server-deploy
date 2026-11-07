//! The session cookie and the extractors that turn it into an authenticated [`Actor`].
//!
//! - [`SignedIn`]: any valid session, including users who must still change their password.
//!   Only for `/me`, `/me/password` and similar.
//! - [`CurrentActor`]: a valid session whose user may act normally. Use this by default.

use axum::{
    extract::FromRequestParts,
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use chrono::TimeDelta;
use gsd_brand::SESSION_COOKIE;
use gsd_core::auth::Actor;

use crate::{http::error::ApiError, state::AppState};

/// A signed-in user, even one who still has to change their password.
#[derive(Debug)]
pub struct SignedIn(pub Actor);

/// A signed-in user who is allowed to use the app normally.
#[derive(Debug)]
pub struct CurrentActor(pub Actor);

impl FromRequestParts<AppState> for SignedIn {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = session_token(&parts.headers).ok_or_else(ApiError::unauthenticated)?;
        let actor = state.auth.authenticate(&token).await?;
        Ok(Self(actor))
    }
}

impl FromRequestParts<AppState> for CurrentActor {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let SignedIn(actor) = SignedIn::from_request_parts(parts, state).await?;
        if actor.user().must_change_password {
            return Err(ApiError::forbidden(
                "password_change_required",
                "Choose a new password first.",
            ));
        }
        Ok(Self(actor))
    }
}

/// The session token from the `Cookie` header, if present.
pub fn session_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value.to_owned())
        .filter(|value| !value.is_empty())
}

/// `Set-Cookie` value for a new session. `HttpOnly` + `SameSite=Strict`; `Secure` behind HTTPS.
pub fn session_cookie(token: &str, ttl: TimeDelta, secure: bool) -> HeaderValue {
    cookie_header(token, ttl.num_seconds(), secure)
}

/// `Set-Cookie` value that removes the session cookie.
pub fn cleared_session_cookie(secure: bool) -> HeaderValue {
    cookie_header("", 0, secure)
}

fn cookie_header(value: &str, max_age_secs: i64, secure: bool) -> HeaderValue {
    let secure = if secure { "; Secure" } else { "" };
    let cookie = format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}{secure}"
    );
    // WHY: the token is URL-safe base64 and the rest is fixed ASCII, so this is always a valid header.
    HeaderValue::from_str(&cookie).unwrap_or_else(|_| HeaderValue::from_static("invalid=; Max-Age=0"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_session_cookie_among_others() {
        let mut headers = HeaderMap::new();
        let value = format!("theme=dark; {SESSION_COOKIE}=abc123; other=x");
        headers.insert(header::COOKIE, HeaderValue::from_str(&value).unwrap());
        assert_eq!(session_token(&headers).as_deref(), Some("abc123"));
    }

    #[test]
    fn empty_or_missing_cookie_is_none() {
        let mut headers = HeaderMap::new();
        assert_eq!(session_token(&headers), None);
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{SESSION_COOKIE}=")).unwrap(),
        );
        assert_eq!(session_token(&headers), None);
    }

    #[test]
    fn cookie_attributes() {
        let cookie = session_cookie("tok", TimeDelta::days(1), true);
        let text = cookie.to_str().unwrap();
        assert!(text.contains("HttpOnly") && text.contains("SameSite=Strict") && text.ends_with("Secure"));
        assert!(text.contains("Max-Age=86400"));
        assert!(
            !session_cookie("tok", TimeDelta::days(1), false)
                .to_str()
                .unwrap()
                .contains("Secure")
        );
    }
}
