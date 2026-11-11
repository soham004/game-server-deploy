//! Outbound HTTP: one shared client and verified downloads.
//!
//! Every request carries the product `User-Agent` (required by `PaperMC` and Modrinth). Downloads
//! stream to a temporary file next to the destination, are checked against the expected hash,
//! and only then renamed into place, so a destination file is never half-written.

use std::{path::Path, time::Duration};

use futures_util::StreamExt;
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

/// Builds the shared HTTP client.
pub fn http_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(gsd_brand::user_agent())
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .build()
}

/// The checksum a downloaded file must match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExpectedHash {
    /// Only where the source publishes nothing stronger (Purpur). Detects corruption, not tampering.
    Md5(String),
    Sha1(String),
    Sha256(String),
    Sha512(String),
    /// The source publishes no checksum (avoid where possible).
    None,
}

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("{url} answered HTTP {status}")]
    Status { url: String, status: u16 },
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum {
        url: String,
        expected: String,
        actual: String,
    },
    #[error("could not write {path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("download cancelled")]
    Cancelled,
}

/// Progress callback: `(bytes_done, total_bytes_if_known)`.
pub type ProgressFn<'a> = &'a (dyn Fn(u64, Option<u64>) + Send + Sync);

/// Downloads `url` to `destination`, verifying `expected` before the file appears.
pub async fn download_verified(
    client: &reqwest::Client,
    url: &str,
    destination: &Path,
    expected: &ExpectedHash,
    cancel: &CancellationToken,
    progress: ProgressFn<'_>,
) -> Result<(), DownloadError> {
    let request_error = |source| DownloadError::Request {
        url: url.to_owned(),
        source,
    };
    let response = client.get(url).send().await.map_err(request_error)?;
    if !response.status().is_success() {
        return Err(DownloadError::Status {
            url: url.to_owned(),
            status: response.status().as_u16(),
        });
    }

    let io_error = |path: &Path| {
        let path = path.display().to_string();
        move |source| DownloadError::Io { path, source }
    };
    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(io_error(parent))?;
    }
    // A name of its own, in a file that must not exist yet: two downloads of the same file
    // (the cache is shared) then never write into each other's, and nothing already there,
    // such as a link, is written through.
    let partial = partial_path(destination).map_err(io_error(destination))?;
    let file = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .await
        .map_err(io_error(&partial))?;

    let written = write_verified(response, file, expected, url, cancel, progress)
        .await
        .map_err(|error| match error {
            Written::Download(error) => error,
            Written::Io(source) => io_error(&partial)(source),
        });
    let finished = match written {
        Ok(()) => tokio::fs::rename(&partial, destination)
            .await
            .map_err(io_error(destination)),
        Err(error) => Err(error),
    };
    if finished.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    finished
}

/// `<destination>.<random>.part`, next to the destination so the final rename is atomic.
fn partial_path(destination: &Path) -> std::io::Result<std::path::PathBuf> {
    let token = crate::random::random_token::<6>().map_err(std::io::Error::other)?;
    let mut name = destination
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(format!(".{token}.part"));
    Ok(destination.with_file_name(name))
}

/// Why a download didn't end up whole in its partial file.
enum Written {
    Download(DownloadError),
    Io(std::io::Error),
}

/// Streams the response into `file`, checking the hash of what arrived.
async fn write_verified(
    response: reqwest::Response,
    mut file: tokio::fs::File,
    expected: &ExpectedHash,
    url: &str,
    cancel: &CancellationToken,
    progress: ProgressFn<'_>,
) -> Result<(), Written> {
    let total = response.content_length();
    let mut hasher = Hasher::for_expected(expected);
    let mut done = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(Written::Download(DownloadError::Cancelled));
        }
        let chunk = chunk.map_err(|source| {
            Written::Download(DownloadError::Request {
                url: url.to_owned(),
                source,
            })
        })?;
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(Written::Io)?;
        done += chunk.len() as u64;
        progress(done, total);
    }
    file.flush().await.map_err(Written::Io)?;
    hasher.verify(expected, url).map_err(Written::Download)
}

enum Hasher {
    Md5(Md5),
    Sha1(Sha1),
    Sha256(Sha256),
    Sha512(Sha512),
    None,
}

impl Hasher {
    fn for_expected(expected: &ExpectedHash) -> Self {
        match expected {
            ExpectedHash::Md5(_) => Self::Md5(Md5::new()),
            ExpectedHash::Sha1(_) => Self::Sha1(Sha1::new()),
            ExpectedHash::Sha256(_) => Self::Sha256(Sha256::new()),
            ExpectedHash::Sha512(_) => Self::Sha512(Sha512::new()),
            ExpectedHash::None => Self::None,
        }
    }

    fn update(&mut self, bytes: &[u8]) {
        match self {
            Self::Md5(h) => h.update(bytes),
            Self::Sha1(h) => h.update(bytes),
            Self::Sha256(h) => h.update(bytes),
            Self::Sha512(h) => h.update(bytes),
            Self::None => {}
        }
    }

    fn verify(self, expected: &ExpectedHash, url: &str) -> Result<(), DownloadError> {
        let (actual, expected) = match (self, expected) {
            (Self::Md5(h), ExpectedHash::Md5(e)) => (hex::encode(h.finalize()), e),
            (Self::Sha1(h), ExpectedHash::Sha1(e)) => (hex::encode(h.finalize()), e),
            (Self::Sha256(h), ExpectedHash::Sha256(e)) => (hex::encode(h.finalize()), e),
            (Self::Sha512(h), ExpectedHash::Sha512(e)) => (hex::encode(h.finalize()), e),
            _ => return Ok(()),
        };
        if actual.eq_ignore_ascii_case(expected.trim()) {
            Ok(())
        } else {
            Err(DownloadError::Checksum {
                url: url.to_owned(),
                expected: expected.clone(),
                actual,
            })
        }
    }
}

/// Hex SHA-256 of a file on disk (used to verify cached downloads).
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut file = std::fs::File::open(path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut HashWriter(&mut hasher))?;
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .map_err(std::io::Error::other)?
}

struct HashWriter<'a, D: Digest>(&'a mut D);

impl<D: Digest> std::io::Write for HashWriter<'_, D> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.update(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    const BODY: &[u8] = b"hello server jar";

    fn sha256_hex(bytes: &[u8]) -> String {
        hex::encode(Sha256::digest(bytes))
    }

    async fn server_with_file() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file.jar"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
            .mount(&server)
            .await;
        server
    }

    #[tokio::test]
    async fn downloads_and_verifies_sha256() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("nested/file.jar");
        let seen = AtomicU64::new(0);

        download_verified(
            &http_client().unwrap(),
            &format!("{}/file.jar", server.uri()),
            &destination,
            &ExpectedHash::Sha256(sha256_hex(BODY)),
            &CancellationToken::new(),
            &|done, _| seen.store(done, Ordering::SeqCst),
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), BODY);
        assert_eq!(seen.load(Ordering::SeqCst), BODY.len() as u64);
        assert_eq!(sha256_file(&destination).await.unwrap(), sha256_hex(BODY));
    }

    #[tokio::test]
    async fn every_hash_kind_is_verified() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let url = format!("{}/file.jar", server.uri());
        let cases = [
            ExpectedHash::Md5(hex::encode(Md5::digest(BODY))),
            ExpectedHash::Sha1(hex::encode(Sha1::digest(BODY))),
            ExpectedHash::Sha256(sha256_hex(BODY)),
            ExpectedHash::Sha512(hex::encode(Sha512::digest(BODY)).to_uppercase()),
        ];
        for (index, good) in cases.into_iter().enumerate() {
            let destination = dir.path().join(format!("ok-{index}.jar"));
            let result = download_verified(
                &http_client().unwrap(),
                &url,
                &destination,
                &good,
                &CancellationToken::new(),
                &|_, _| {},
            )
            .await;
            assert!(result.is_ok(), "{good:?}: {result:?}");

            let wrong = match good {
                ExpectedHash::Md5(_) => ExpectedHash::Md5("0".repeat(32)),
                ExpectedHash::Sha1(_) => ExpectedHash::Sha1("0".repeat(40)),
                ExpectedHash::Sha256(_) => ExpectedHash::Sha256("0".repeat(64)),
                ExpectedHash::Sha512(_) | ExpectedHash::None => ExpectedHash::Sha512("0".repeat(128)),
            };
            let bad = dir.path().join(format!("bad-{index}.jar"));
            let result = download_verified(
                &http_client().unwrap(),
                &url,
                &bad,
                &wrong,
                &CancellationToken::new(),
                &|_, _| {},
            )
            .await;
            assert!(matches!(result, Err(DownloadError::Checksum { .. })), "{wrong:?}");
            assert!(!bad.exists());
        }
    }

    #[tokio::test]
    async fn checksum_mismatch_leaves_no_file() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("file.jar");

        let result = download_verified(
            &http_client().unwrap(),
            &format!("{}/file.jar", server.uri()),
            &destination,
            &ExpectedHash::Sha1("0000".into()),
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await;

        assert!(matches!(result, Err(DownloadError::Checksum { .. })));
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            0,
            "neither the file nor its partial copy is left"
        );
    }

    /// Two servers installing the same jar download it into the shared cache at once. Each must
    /// write its own partial file: sharing one let the second keep writing into the file the
    /// first had already checked and moved into place.
    #[tokio::test]
    async fn downloads_of_the_same_file_at_once_do_not_share_a_partial_file() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("cached.jar");
        let url = format!("{}/file.jar", server.uri());
        let client = http_client().unwrap();
        let expected = ExpectedHash::Sha256(sha256_hex(BODY));
        let cancel = CancellationToken::new();

        let downloads =
            (0..8).map(|_| download_verified(&client, &url, &destination, &expected, &cancel, &|_, _| {}));
        let results = futures_util::future::join_all(downloads).await;

        assert!(results.iter().all(Result::is_ok), "{results:?}");
        assert_eq!(std::fs::read(&destination).unwrap(), BODY);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    /// A link where a partial file used to be named (`<name>.part`) is not written through.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_link_planted_at_the_old_partial_name_is_left_alone() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let outside = dir.path().join("outside.txt");
        std::fs::write(&outside, "untouched").unwrap();
        let folder = dir.path().join("server");
        std::fs::create_dir(&folder).unwrap();
        std::os::unix::fs::symlink(&outside, folder.join("server.part")).unwrap();

        download_verified(
            &http_client().unwrap(),
            &format!("{}/file.jar", server.uri()),
            &folder.join("server.jar"),
            &ExpectedHash::None,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await
        .unwrap();

        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "untouched");
        assert_eq!(std::fs::read(folder.join("server.jar")).unwrap(), BODY);
    }

    #[tokio::test]
    async fn http_errors_are_reported() {
        let server = MockServer::start().await;
        let dir = tempfile::tempdir().unwrap();
        let result = download_verified(
            &http_client().unwrap(),
            &format!("{}/missing", server.uri()),
            &dir.path().join("x"),
            &ExpectedHash::None,
            &CancellationToken::new(),
            &|_, _| {},
        )
        .await;
        assert!(matches!(result, Err(DownloadError::Status { status: 404, .. })));
    }

    #[tokio::test]
    async fn cancelled_downloads_stop() {
        let server = server_with_file().await;
        let dir = tempfile::tempdir().unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = download_verified(
            &http_client().unwrap(),
            &format!("{}/file.jar", server.uri()),
            &dir.path().join("file.jar"),
            &ExpectedHash::None,
            &cancel,
            &|_, _| {},
        )
        .await;
        assert!(matches!(result, Err(DownloadError::Cancelled)));
    }
}
