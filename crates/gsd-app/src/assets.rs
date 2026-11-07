//! The web UI from `web/`.
//!
//! Release builds embed the files in the binary. Debug builds read them from disk, so frontend
//! edits show up on reload without recompiling. HTML files get `{{APP_NAME}}` and
//! `{{APP_VERSION}}` filled in from `gsd_brand`; the frontend never hard-codes the name.

use std::fmt::Write as _;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::{EmbeddedFile, RustEmbed};

#[derive(RustEmbed)]
#[folder = "../../web/"]
struct WebAssets;

const INDEX: &str = "index.html";

/// Fallback handler for every non-API path.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }

    let path = match uri.path().trim_start_matches('/') {
        "" => INDEX,
        path => path,
    };
    if path.split('/').any(|segment| segment == "..") {
        return StatusCode::NOT_FOUND.into_response();
    }

    if let Some(file) = WebAssets::get(path) {
        return respond(path, &file, &headers);
    }
    // Client-side routes (e.g. /instances/abc) get the app shell; missing files stay 404.
    if is_client_route(path)
        && let Some(index) = WebAssets::get(INDEX)
    {
        return respond(INDEX, &index, &headers);
    }
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

fn is_client_route(path: &str) -> bool {
    !path.rsplit('/').next().unwrap_or_default().contains('.')
}

fn respond(path: &str, file: &EmbeddedFile, request_headers: &HeaderMap) -> Response {
    let etag = etag_for(file);
    let not_modified = request_headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|value| value.as_bytes() == etag.as_bytes());

    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        let body = if mime.subtype() == mime_guess::mime::HTML {
            render_template(&file.data)
        } else {
            file.data.to_vec()
        };
        let mut response = Body::from(body).into_response();
        if let Ok(content_type) = HeaderValue::from_str(mime.as_ref()) {
            response.headers_mut().insert(header::CONTENT_TYPE, content_type);
        }
        response
    };

    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    if let Ok(value) = HeaderValue::from_str(&etag) {
        headers.insert(header::ETAG, value);
    }
    response
}

/// Content hash plus app version (templated HTML differs between builds).
fn etag_for(file: &EmbeddedFile) -> String {
    let mut tag = String::from("\"");
    for byte in file.metadata.sha256_hash().iter().take(16) {
        let _ = write!(tag, "{byte:02x}");
    }
    let _ = write!(tag, "-{}\"", gsd_brand::VERSION);
    tag
}

fn render_template(html: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(html)
        .replace("{{APP_NAME}}", &escape_html(gsd_brand::NAME))
        .replace("{{APP_VERSION}}", &escape_html(gsd_brand::VERSION))
        .into_bytes()
}

fn escape_html(text: &str) -> String {
    text.chars()
        .fold(String::with_capacity(text.len()), |mut out, c| {
            match c {
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                other => out.push(other),
            }
            out
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_routes_have_no_file_extension() {
        assert!(is_client_route("instances/01ABC"));
        assert!(is_client_route("settings"));
        assert!(!is_client_route("js/main.js"));
        assert!(!is_client_route("favicon.ico"));
    }

    #[test]
    fn templating_escapes_the_name() {
        assert_eq!(escape_html("A&B <x>"), "A&amp;B &lt;x&gt;");
        let out = String::from_utf8(render_template(b"<title>{{APP_NAME}}</title>")).unwrap();
        assert!(out.contains(&escape_html(gsd_brand::NAME)));
        assert!(!out.contains("{{"));
    }
}
