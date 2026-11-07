//! Who is calling: client IP, whether the original request was HTTPS, and the user agent.
//!
//! `X-Forwarded-For` / `X-Forwarded-Proto` are honoured **only** when the direct peer is a
//! configured trusted proxy (e.g. the nginx container); otherwise anyone could spoof them.

use std::{convert::Infallible, net::IpAddr, net::SocketAddr};

use axum::{
    extract::{ConnectInfo, FromRequestParts},
    http::{HeaderMap, header, request::Parts},
};
use gsd_core::auth::ClientInfo;
use ipnet::IpNet;

use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct ClientContext {
    pub ip: Option<IpAddr>,
    /// True when the browser reached us over HTTPS (via a trusted TLS-terminating proxy).
    pub is_https: bool,
    pub user_agent: Option<String>,
}

impl ClientContext {
    pub fn info(&self) -> ClientInfo {
        ClientInfo {
            ip: self.ip,
            user_agent: self.user_agent.clone(),
        }
    }
}

impl FromRequestParts<AppState> for ClientContext {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let peer = parts
            .extensions
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(addr)| addr.ip());
        let (ip, is_https) = resolve(peer, &parts.headers, &state.config.trusted_proxies);
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .map(|agent| agent.chars().take(256).collect());
        Ok(Self {
            ip,
            is_https,
            user_agent,
        })
    }
}

/// Resolves the real client address and scheme from the peer address and proxy headers.
fn resolve(peer: Option<IpAddr>, headers: &HeaderMap, trusted: &[IpNet]) -> (Option<IpAddr>, bool) {
    let is_trusted = |ip: &IpAddr| trusted.iter().any(|net| net.contains(ip));
    let Some(peer) = peer.filter(is_trusted) else {
        return (peer, false);
    };

    // Walk the chain right-to-left: the first hop that isn't one of our proxies is the client.
    let forwarded_for = header_str(headers, "x-forwarded-for").unwrap_or_default();
    let client = forwarded_for
        .split(',')
        .rev()
        .filter_map(|hop| hop.trim().parse::<IpAddr>().ok())
        .find(|hop| !is_trusted(hop))
        .unwrap_or(peer);

    let proto = header_str(headers, "x-forwarded-proto").and_then(|value| value.split(',').next());
    let is_https = proto.is_some_and(|scheme| scheme.trim().eq_ignore_ascii_case("https"));
    (Some(client), is_https)
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn untrusted_peers_cannot_spoof_headers() {
        let peer: IpAddr = "203.0.113.9".parse().unwrap();
        let spoofed = headers(&[("x-forwarded-for", "1.2.3.4"), ("x-forwarded-proto", "https")]);
        assert_eq!(resolve(Some(peer), &spoofed, &[]), (Some(peer), false));
    }

    #[test]
    fn trusted_proxy_reveals_client_and_scheme() {
        let proxy: IpAddr = "172.18.0.2".parse().unwrap();
        let trusted = ["172.16.0.0/12".parse().unwrap()];
        let forwarded = headers(&[
            ("x-forwarded-for", "198.51.100.7, 172.18.0.5"),
            ("x-forwarded-proto", "https"),
        ]);
        let (ip, https) = resolve(Some(proxy), &forwarded, &trusted);
        assert_eq!(ip, Some("198.51.100.7".parse().unwrap()));
        assert!(https);
    }

    #[test]
    fn missing_peer_is_unknown() {
        assert_eq!(resolve(None, &HeaderMap::new(), &[]), (None, false));
    }
}
