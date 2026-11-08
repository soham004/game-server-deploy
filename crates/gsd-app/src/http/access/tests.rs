use std::collections::BTreeSet;

use super::*;

fn get(path: &str) -> Verdict {
    verdict(&Method::GET, path, None, false)
}

fn redirect(to: &str) -> Verdict {
    Verdict::Redirect(to.to_owned())
}

#[test]
fn only_sign_in_and_health_are_open_on_the_api() {
    let post = |path| verdict(&Method::POST, path, None, false);
    assert_eq!(post("/api/v1/auth/login"), Verdict::Allow);
    assert_eq!(post("/api/v1/auth/logout"), Verdict::Allow);
    assert_eq!(get("/api/v1/health"), Verdict::Allow);

    for refused in [
        get("/api/v1/meta"),
        get("/api/v1/me"),
        get("/api/v1/instances"),
        get("/api/v1/ws"),
        get("/api/v1/auth/login"), // the wrong method is refused, not 405
        post("/api/v1/health"),
        post("/api/v1/instances"),
        get("/api/v1/no-such-endpoint"), // not 404: outsiders can't map the API
        get("/api/v1/setup/status"),     // set up already
        post("/api/v1/setup"),
        verdict(&Method::DELETE, "/api/v1/users/x", None, false),
        get("/api/v1/auth/login/../../instances"),
        get("/api/v1//auth/login"),
        get("/api/v2/health"),
    ] {
        assert_eq!(refused, Verdict::Refuse);
    }
}

#[test]
fn setup_is_open_only_until_the_first_account_exists() {
    let fresh = |method: &Method, path| verdict(method, path, None, true);
    assert_eq!(fresh(&Method::GET, "/api/v1/setup/status"), Verdict::Allow);
    assert_eq!(fresh(&Method::POST, "/api/v1/setup"), Verdict::Allow);
    assert_eq!(fresh(&Method::GET, "/setup.html"), Verdict::Allow);
    assert_eq!(fresh(&Method::GET, "/js/pages/setup.js"), Verdict::Allow);
    // Nobody to sign in yet: every page leads to setup.
    for page in ["/", "/login.html", "/servers/abc", "/index.html"] {
        assert_eq!(fresh(&Method::GET, page), redirect("/setup.html"), "{page}");
    }
    assert_eq!(fresh(&Method::GET, "/js/pages/login.js"), Verdict::Refuse);
    assert_eq!(fresh(&Method::GET, "/js/main.js"), Verdict::Refuse);
    assert_eq!(fresh(&Method::GET, "/api/v1/instances"), Verdict::Refuse);

    // Afterwards the setup page leads to sign-in, and its own script is closed.
    assert_eq!(get("/setup.html"), redirect("/login.html"));
    assert_eq!(get("/js/pages/setup.js"), Verdict::Refuse);
}

#[test]
fn pages_lead_to_sign_in_and_other_files_are_refused() {
    assert_eq!(get("/login.html"), Verdict::Allow);
    for file in SIGN_IN_FILES {
        assert_eq!(get(&format!("/{file}")), Verdict::Allow, "{file}");
    }
    assert_eq!(get("/"), redirect("/login.html"));
    assert_eq!(get("/index.html"), redirect("/login.html?next=%2Findex.html"));
    assert_eq!(get("/users"), redirect("/login.html?next=%2Fusers"));
    assert_eq!(
        verdict(&Method::GET, "/servers/01ABC", Some("tab=files&x=1"), false),
        redirect("/login.html?next=%2Fservers%2F01ABC%3Ftab%3Dfiles%26x%3D1"),
    );
    for refused in [
        "/js/main.js",
        "/js/views/users.js",
        "/css/layout.css",
        "/favicon.ico",
        "/missing.js",
    ] {
        assert_eq!(get(refused), Verdict::Refuse, "{refused}");
    }
    // Only reading: a POST to a page is refused rather than redirected.
    assert_eq!(
        verdict(&Method::POST, "/login.html", None, false),
        Verdict::Refuse
    );
    assert_eq!(verdict(&Method::HEAD, "/login.html", None, false), Verdict::Allow);
}

#[test]
fn tricks_with_the_path_do_not_open_anything() {
    for path in [
        "/login.html/../js/main.js",
        "/js/pages/../main.js",
        "/%6Cogin.html",
        "/login.html%00.js",
        "//login.html",
        "/LOGIN.HTML",
        "/js/core/api.js/",
    ] {
        assert_ne!(get(path), Verdict::Allow, "{path}");
    }
}

#[test]
fn the_return_path_is_encoded_as_one_query_value() {
    assert_eq!(encode_component("/a b?c=d&e"), "%2Fa%20b%3Fc%3Dd%26e");
    assert_eq!(encode_component("/ü"), "%2F%C3%BC");
    assert_eq!(encode_component("AZaz09-._~"), "AZaz09-._~");
}

/// The allowlists must be exactly what the pages load: a missing file breaks the page for
/// everyone signed out, and an extra one is open for no reason.
#[test]
fn the_open_files_are_exactly_what_the_pages_load() {
    for (page, files) in [("login.html", SIGN_IN_FILES), ("setup.html", SETUP_FILES)] {
        let listed: BTreeSet<String> = files.iter().map(|file| (*file).to_owned()).collect();
        assert_eq!(listed.len(), files.len(), "{page}: listed twice");
        assert_eq!(loaded_by(page), listed, "{page}");
    }
}

/// `page` and everything it loads, following stylesheets and module imports.
fn loaded_by(page: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut queue = vec![page.to_owned()];
    while let Some(file) = queue.pop() {
        if !found.insert(file.clone()) {
            continue;
        }
        let bytes = crate::assets::contents(&file).unwrap_or_else(|| panic!("{file} is not in web/"));
        let text = String::from_utf8(bytes).unwrap();
        queue.extend(references(&file, &text));
    }
    found
}

/// The site files `text` (the contents of `file`) refers to, as paths under `web/`.
fn references(file: &str, text: &str) -> Vec<String> {
    let quoted = |prefix: &str, quotes: &[char]| -> Vec<String> {
        let mut out = Vec::new();
        for quote in quotes {
            let marker = format!("{prefix}{quote}");
            for (index, _) in text.match_indices(&marker) {
                let rest = &text[index + marker.len()..];
                if let Some(end) = rest.find(*quote) {
                    out.push(rest[..end].to_owned());
                }
            }
        }
        out
    };
    let extension = std::path::Path::new(file)
        .extension()
        .and_then(|extension| extension.to_str());
    let mut targets = Vec::new();
    if extension == Some("html") {
        targets.extend(quoted("src=", &['"', '\'']));
        targets.extend(quoted("href=", &['"', '\'']));
    }
    if extension == Some("js") {
        targets.extend(quoted("from ", &['\'', '"']));
        targets.extend(quoted("import ", &['\'', '"']));
        targets.extend(quoted("import(", &['\'', '"']));
    }
    if extension == Some("css") {
        targets.extend(quoted("url(", &['"', '\'']));
        targets.extend(quoted("@import ", &['"', '\'']));
    }
    targets
        .into_iter()
        .filter(|target| !target.contains("://") && !target.starts_with('#') && !target.starts_with("data:"))
        .map(|target| resolve(file, &target))
        .collect()
}

/// `target` as written in `from` (absolute from the site root, or relative), as a path under `web/`.
fn resolve(from: &str, target: &str) -> String {
    let mut parts: Vec<&str> = if target.starts_with('/') {
        Vec::new()
    } else {
        let mut dir: Vec<&str> = from.split('/').collect();
        dir.pop();
        dir
    };
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[test]
fn references_resolve_relative_and_absolute_paths() {
    assert_eq!(resolve("js/pages/login.js", "../core/api.js"), "js/core/api.js");
    assert_eq!(resolve("js/pages/login.js", "./x.js"), "js/pages/x.js");
    assert_eq!(resolve("login.html", "/css/base.css"), "css/base.css");
    let html = r#"<link href="/a.css"><script src='/b.js'></script><a href="https://x.test">"#;
    assert_eq!(references("p.html", html), ["b.js", "a.css"]);
}
