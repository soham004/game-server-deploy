//! Maven repositories (Forge, `NeoForge`, Quilt installers): checksum side files and the
//! `maven-metadata.xml` release pointer.

use gsd_core::net::ExpectedHash;

use super::{Artifact, CatalogError, Fetcher};

/// `url`, verified against the SHA-1 Maven publishes next to it (`<url>.sha1`).
pub(super) async fn artifact_with_sha1(fetcher: &Fetcher, url: String) -> Result<Artifact, CatalogError> {
    let sidecar = format!("{url}.sha1");
    let text = fetcher.text(&sidecar).await?;
    let sha1 = parse_sha1(&text).ok_or_else(|| CatalogError::Malformed {
        url: sidecar,
        detail: "not a SHA-1 checksum".into(),
    })?;
    Ok(Artifact {
        url,
        hash: ExpectedHash::Sha1(sha1),
    })
}

/// The `<release>` (else `<latest>`) version from a `maven-metadata.xml`.
pub(super) fn release_version(xml: &str) -> Option<String> {
    element(xml, "release")
        .or_else(|| element(xml, "latest"))
        .map(str::to_owned)
}

/// Side files hold the hex digest, sometimes followed by the file name.
fn parse_sha1(text: &str) -> Option<String> {
    let token = text.split_whitespace().next()?;
    (token.len() == 40 && token.chars().all(|c| c.is_ascii_hexdigit())).then(|| token.to_ascii_lowercase())
}

fn element<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml[start..].find(&format!("</{name}>"))?;
    Some(xml[start..end].trim()).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_release_from_real_quilt_metadata() {
        let xml = include_str!("../../tests/fixtures/quilt_installer_metadata.xml");
        assert_eq!(release_version(xml).as_deref(), Some("0.15.1"));
        assert_eq!(
            release_version("<metadata><versioning><latest>2.0</latest></versioning></metadata>").as_deref(),
            Some("2.0")
        );
        assert_eq!(release_version("<metadata><release> </release></metadata>"), None);
        assert_eq!(release_version("garbage"), None);
    }

    #[test]
    fn parses_sha1_side_files() {
        let hash = "0123456789abcdef0123456789ABCDEF01234567";
        assert_eq!(parse_sha1(hash).unwrap(), hash.to_ascii_lowercase());
        assert_eq!(
            parse_sha1(&format!("{hash}  forge-installer.jar\n")).unwrap(),
            hash.to_ascii_lowercase()
        );
        assert_eq!(parse_sha1("<html>404</html>"), None);
        assert_eq!(parse_sha1(""), None);
    }
}
