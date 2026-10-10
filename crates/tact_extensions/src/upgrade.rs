//! Self-upgrade support for the `tact upgrade` subcommand.
//!
//! Checks the latest GitHub release for the platform, downloads the matching
//! prebuilt archive, verifies it against the published `SHA256SUMS`, and
//! atomically replaces the currently running binary. Linux and macOS replace
//! the executable in place (the running process keeps its old inode); Windows
//! is not yet supported in-place and the command points users at
//! `scripts/install.ps1`.

use std::{
    cmp::Ordering,
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};

/// Options accepted by the `tact upgrade` subcommand.
#[derive(Debug, Clone)]
pub struct UpgradeOptions {
    /// GitHub repository in `owner/name` form.
    pub repo: String,
    /// Skip the interactive confirmation prompt.
    pub yes: bool,
    /// Check for a newer version and print it without upgrading.
    pub check: bool,
}

const USER_AGENT: &str = concat!("tact-upgrade/", env!("CARGO_PKG_VERSION"));
const GITHUB_API: &str = "https://api.github.com";
const BINARY_NAME: &str = "tact-ui";

/// Runs the `upgrade` subcommand.
pub async fn run_upgrade(options: UpgradeOptions) -> Result<()> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
        .context("failed to build HTTP client")?;

    let current = current_version();
    println!("Current version: {current}");

    ensure_platform_supported()?;
    let triple = target_triple()?;

    let latest =
        match find_latest_release_with_asset(&client, GITHUB_API, &options.repo, &triple).await? {
            Some(version) => version,
            None => {
                println!(
                    "No release with a binary for {triple} found in {}.",
                    options.repo
                );
                println!("Install from source instead: scripts/install.sh --from-source");
                return Ok(());
            }
        };
    println!("Latest release:  {latest}");

    match compare_versions(&latest, &current) {
        Ordering::Less | Ordering::Equal => {
            println!("Already up to date ({current}).");
            return Ok(());
        }
        Ordering::Greater => {}
    }

    if options.check {
        println!("A newer version is available: {latest} (current: {current}).");
        println!("Run `tact upgrade` to install it.");
        return Ok(());
    }

    if !options.yes && !confirm(&format!("Upgrade from {current} to {latest}? [y/N] "))? {
        println!("Upgrade cancelled.");
        return Ok(());
    }

    let asset_name = format!("{BINARY_NAME}-v{latest}-{triple}.tar.gz");
    let asset_url = format!(
        "https://github.com/{}/releases/download/v{latest}/{asset_name}",
        options.repo
    );

    println!("Downloading {asset_name} ...");
    let bytes = download(&client, &asset_url).await?;
    println!("Downloaded {} bytes.", bytes.len());

    if let Some(expected) = fetch_sha256(&client, &options.repo, &latest, &asset_name).await? {
        println!("Verifying SHA-256 checksum ...");
        let actual = hex_sha256(&bytes);
        if !actual.eq_ignore_ascii_case(&expected) {
            bail!("checksum mismatch for {asset_name}: expected {expected}, got {actual}");
        }
        println!("Checksum OK.");
    } else {
        println!("warning: no SHA256SUMS asset found; skipping checksum verification");
    }

    let exe = std::env::current_exe().context("failed to locate current executable")?;
    let exe_dir = exe
        .parent()
        .context("current executable has no parent directory")?
        .to_path_buf();

    println!("Extracting {BINARY_NAME} ...");
    // Extract next to the executable so the final rename is atomic (same fs).
    let temp_dir = exe_dir.join(format!(".tact-upgrade-{}", std::process::id()));
    fs::create_dir_all(&temp_dir).with_context(|| {
        format!(
            "failed to create temporary directory {}",
            temp_dir.display()
        )
    })?;
    let result = (|| -> Result<()> {
        let extracted = extract_tar_gz(&bytes, &temp_dir, BINARY_NAME)?;
        replace_current_binary(&extracted, &exe)?;
        Ok(())
    })();
    let _ = fs::remove_dir_all(&temp_dir);

    result?;
    println!("Upgraded to {latest}.");
    println!("Restart tact-ui to use the new version.");
    Ok(())
}

/// The version of the running binary (workspace-managed, shared by `tact` and
/// `tact-ui`).
fn current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Finds the newest non-draft, non-prerelease release whose assets include a
/// binary archive for `triple` (e.g. `aarch64-apple-darwin`). Returns the
/// release version without the leading `v`, or `None` when no usable release
/// exists. Scanning the release list (instead of `/releases/latest`) lets
/// `upgrade` skip tags that were published without build assets.
async fn find_latest_release_with_asset(
    client: &reqwest::Client,
    api_base: &str,
    repo: &str,
    triple: &str,
) -> Result<Option<String>> {
    let asset_suffix = format!("-{triple}.tar.gz");
    let url = format!("{api_base}/repos/{repo}/releases?per_page=100");
    let response = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to query GitHub releases API: {url}"))?;

    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let status = response.status();
    if !status.is_success() {
        bail!("GitHub releases API returned {status} for {url}");
    }
    let releases: Vec<serde_json::Value> = response
        .json()
        .await
        .context("failed to parse GitHub releases API response")?;

    for release in releases {
        if release
            .get("draft")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        if release
            .get("prerelease")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        let tag = release
            .get("tag_name")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let has_asset = release
            .get("assets")
            .and_then(|value| value.as_array())
            .map(|assets| {
                assets.iter().any(|asset| {
                    asset
                        .get("name")
                        .and_then(|name| name.as_str())
                        .is_some_and(|name| name.ends_with(&asset_suffix))
                })
            })
            .unwrap_or(false);
        if has_asset {
            return Ok(Some(tag.trim_start_matches('v').to_string()));
        }
    }
    Ok(None)
}

/// Fetches the published `SHA256SUMS` for a release and returns the expected
/// checksum for `asset_name`. Returns `None` when the checksum file is absent
/// (some self-hosted mirrors do not publish one).
async fn fetch_sha256(
    client: &reqwest::Client,
    repo: &str,
    version: &str,
    asset_name: &str,
) -> Result<Option<String>> {
    let url = format!("https://github.com/{repo}/releases/download/v{version}/SHA256SUMS");
    let response = match client.get(&url).send().await {
        Ok(response) => response,
        Err(_) => return Ok(None),
    };
    if !response.status().is_success() {
        return Ok(None);
    }
    let body = response.text().await.context("failed to read SHA256SUMS")?;
    Ok(sha256_from_sums(&body, asset_name))
}

/// Extracts the checksum for `asset_name` from a `sha256sum`-style listing.
fn sha256_from_sums(body: &str, asset_name: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?;
        if name == asset_name {
            Some(hash.to_string())
        } else {
            None
        }
    })
}

/// Downloads a URL into memory with a clear error on failure.
async fn download(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    let status = response.status();
    if !status.is_success() {
        bail!("download failed: {status} for {url}");
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .with_context(|| format!("failed to read download body from {url}"))
}

/// Extracts a file named `wanted` from an in-memory `.tar.gz` archive into
/// `dir`, returning the path of the extracted file.
fn extract_tar_gz(bytes: &[u8], dir: &Path, wanted: &str) -> Result<PathBuf> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut found = None;
    for entry in archive.entries().context("failed to read tar archive")? {
        let mut entry = entry.context("failed to read tar entry")?;
        let entry_path = entry.path().context("failed to read tar entry path")?;
        let file_name = entry_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        if file_name != wanted {
            continue;
        }
        let dest = dir.join(wanted);
        let mut out = fs::File::create(&dest)
            .with_context(|| format!("failed to create {}", dest.display()))?;
        std::io::copy(&mut entry, &mut out).context("failed to extract binary")?;
        out.flush()?;
        found = Some(dest);
        break;
    }
    found.ok_or_else(|| anyhow!("release archive did not contain {wanted}"))
}

/// Atomically replaces `exe` with `new` (must live on the same filesystem).
#[cfg(unix)]
fn replace_current_binary(new: &Path, exe: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut perms = fs::metadata(new)
        .with_context(|| format!("failed to stat {}", new.display()))?
        .permissions();
    perms.set_mode(0o755);
    fs::set_permissions(new, perms)
        .with_context(|| format!("failed to chmod {}", new.display()))?;

    fs::rename(new, exe).with_context(|| {
        format!(
            "failed to replace {} (is its directory writable? run with sudo, or reinstall via scripts/install.sh)",
            exe.display()
        )
    })?;
    Ok(())
}

/// In-place replacement of a running executable is not possible on Windows.
#[cfg(not(unix))]
fn replace_current_binary(_new: &Path, _exe: &Path) -> Result<()> {
    bail!(
        "in-place self-upgrade is not supported on this platform; re-run scripts/install.ps1 to upgrade"
    )
}

/// The release asset target triple for the current platform.
fn target_triple() -> Result<String> {
    let triple = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-gnu",
        ("linux", "aarch64") => "aarch64-unknown-linux-gnu",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => bail!("unsupported platform for self-upgrade: {os}-{arch}"),
    };
    Ok(triple.to_string())
}

/// Fail fast on platforms where in-place replacement is not supported, before
/// any download happens.
#[cfg(unix)]
fn ensure_platform_supported() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_platform_supported() -> Result<()> {
    bail!(
        "in-place self-upgrade is not supported on Windows; re-run scripts/install.ps1 to upgrade"
    )
}

/// Compares dotted-numeric version strings like `1.0.10` (optionally with a
/// leading `v`). Missing trailing components compare as zero (`1.2 == 1.2.0`).
fn compare_versions(a: &str, b: &str) -> Ordering {
    fn components(value: &str) -> Vec<u64> {
        value
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.trim().parse::<u64>().unwrap_or(0))
            .collect()
    }

    let mut left = components(a);
    let mut right = components(b);
    let max = left.len().max(right.len());
    left.resize(max, 0);
    right.resize(max, 0);
    left.cmp(&right)
}

fn hex_sha256(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Prompts on stdin for a y/N answer. Returns `Ok(true)` only for y/yes.
fn confirm(prompt: &str) -> Result<bool> {
    print!("{prompt}");
    let _ = std::io::stdout().flush();

    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("failed to read confirmation")?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compare_versions_orders_dotted_numeric() {
        assert_eq!(compare_versions("1.0.10", "1.0.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.9", "1.0.10"), Ordering::Less);
        assert_eq!(compare_versions("v1.0.10", "1.0.10"), Ordering::Equal);
        assert_eq!(compare_versions("1.2", "1.2.0"), Ordering::Equal);
        assert_eq!(compare_versions("2.0.0", "1.9.9"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.10", "1.0.10"), Ordering::Equal);
    }

    #[test]
    fn hex_sha256_matches_known_vector() {
        assert_eq!(
            hex_sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn sha256_from_sums_finds_matching_asset() {
        let body = "abc123  tact-ui-v1.0.11-aarch64-apple-darwin.tar.gz\ndef456  tact-ui-v1.0.11-x86_64-unknown-linux-gnu.tar.gz\n";
        assert_eq!(
            sha256_from_sums(body, "tact-ui-v1.0.11-aarch64-apple-darwin.tar.gz").as_deref(),
            Some("abc123")
        );
        assert_eq!(
            sha256_from_sums(body, "tact-ui-v1.0.11-missing.tar.gz"),
            None
        );
    }

    #[test]
    fn target_triple_matches_published_platforms() {
        let triple = target_triple().expect("platform should be supported");
        let known = [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
            "x86_64-pc-windows-msvc",
        ];
        assert!(
            known.contains(&triple.as_str()),
            "unexpected triple {triple}"
        );
    }

    fn make_tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, *name, *content).unwrap();
        }
        let raw = builder.into_inner().unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&raw).unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn extract_tar_gz_finds_binary() {
        let archive = make_tar_gz(&[("tact-ui", b"new-binary"), ("README.md", b"readme")]);
        let dir = tempfile::tempdir().unwrap();
        let extracted = extract_tar_gz(&archive, dir.path(), "tact-ui").unwrap();
        assert_eq!(fs::read(&extracted).unwrap(), b"new-binary");

        let missing = extract_tar_gz(&archive, dir.path(), "nope");
        assert!(missing.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn replace_current_binary_replaces_and_chmods() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("tact-ui");
        let new = dir.path().join("tact-ui.new");
        fs::write(&exe, b"old").unwrap();
        fs::write(&new, b"new").unwrap();

        replace_current_binary(&new, &exe).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new");
        assert!(!new.exists());
        let mode = fs::metadata(&exe).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[tokio::test]
    async fn find_latest_release_skips_assetless_and_prerelease_releases() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/repos/rust-infra/tact/releases"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    // Newest tag but no build assets → must be skipped.
                    {"tag_name": "v1.1.1", "draft": false, "prerelease": false, "assets": []},
                    // Prerelease with a matching asset → must be skipped.
                    {"tag_name": "v1.1.0-rc.1", "draft": false, "prerelease": true, "assets": [
                        {"name": "tact-ui-v1.1.0-rc.1-aarch64-apple-darwin.tar.gz"}
                    ]},
                    // First usable release.
                    {"tag_name": "v1.1.0", "draft": false, "prerelease": false, "assets": [
                        {"name": "SHA256SUMS"},
                        {"name": "tact-ui-v1.1.0-aarch64-apple-darwin.tar.gz"}
                    ]},
                    {"tag_name": "v1.0.9", "draft": false, "prerelease": false, "assets": [
                        {"name": "tact-ui-v1.0.9-x86_64-unknown-linux-gnu.tar.gz"}
                    ]}
                ])),
            )
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let version = find_latest_release_with_asset(
            &client,
            &server.uri(),
            "rust-infra/tact",
            "aarch64-apple-darwin",
        )
        .await
        .unwrap();
        assert_eq!(version.as_deref(), Some("1.1.0"));

        // A triple with no matching asset anywhere → None.
        let none = find_latest_release_with_asset(
            &client,
            &server.uri(),
            "rust-infra/tact",
            "mips-unknown-linux-gnu",
        )
        .await
        .unwrap();
        assert!(none.is_none());
    }

    #[tokio::test]
    async fn find_latest_release_returns_none_when_no_releases() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/repos/rust-infra/tact/releases"))
            .respond_with(wiremock::ResponseTemplate::new(404))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        let version = find_latest_release_with_asset(
            &client,
            &server.uri(),
            "rust-infra/tact",
            "aarch64-apple-darwin",
        )
        .await
        .unwrap();
        assert!(version.is_none());
    }

    #[tokio::test]
    async fn download_returns_bytes_and_errors_on_404() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path("/asset.bin"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_bytes(b"payload"))
            .mount(&server)
            .await;

        let client = reqwest::Client::builder().build().unwrap();
        assert_eq!(
            download(&client, &format!("{}/asset.bin", server.uri()))
                .await
                .unwrap(),
            b"payload"
        );
        assert!(
            download(&client, &format!("{}/missing", server.uri()))
                .await
                .is_err()
        );
    }
}
