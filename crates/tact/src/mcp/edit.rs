//! The write side of Tact's native `mcp.json`.
//!
//! [`McpConfigFile`](super::McpConfigFile) only reads. `mcp add` must write
//! without disturbing anything Tact does not model, so this module edits the
//! raw JSON document rather than round-tripping through the typed structs:
//! unknown top-level keys and unknown keys on *other* servers survive, because
//! a field Tact does not understand yet must not be deleted just because the
//! user added a second server.
//!
//! What is **not** preserved is formatting: the document is re-serialized
//! through `serde_json`, so key order (sorted), indentation, line endings and
//! comments-in-string trivia are normalized. Only key *presence* and value
//! contents are guaranteed.
//!
//! Note: never log header *values* or environment values — they commonly carry
//! secrets. Log server names, paths, URLs and header *names* only.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use http::{HeaderName, HeaderValue};
use serde_json::{Map, Value, json};

use super::McpTransportKind;
use crate::consts::TactPath;

/// Which `mcp.json` an edit targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpConfigScope {
    /// `<workdir>/.tact/mcp.json` — wins over the user file by server name.
    Project,
    /// `$HOME/.tact/mcp.json` — shared by every project.
    User,
}

impl McpConfigScope {
    /// The file this scope writes; the user scope needs `$HOME`.
    pub fn path(self, workdir: &Path) -> Result<PathBuf> {
        match self {
            Self::Project => Ok(TactPath::new(workdir).mcp_config_path()),
            Self::User => TactPath::home_mcp_config_path()
                .context("cannot locate the user mcp.json: HOME is not set"),
        }
    }
}

/// Transport of a server being added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpDraftTransport {
    /// Local subprocess over stdio.
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
    },
    /// Remote Streamable HTTP, optionally OAuth-authorized.
    Remote {
        url: String,
        headers: BTreeMap<String, String>,
        oauth: bool,
    },
}

/// One server to be added, validated before anything touches disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServerDraft {
    pub name: String,
    pub transport: McpDraftTransport,
}

impl McpServerDraft {
    /// Validates a draft built from CLI arguments.
    ///
    /// Validation is deliberately strict here rather than at connect time: a
    /// header that Tact would silently drop or a URL it cannot route is much
    /// cheaper to reject while the user is still looking at the command.
    pub fn new(name: impl Into<String>, transport: McpDraftTransport) -> Result<Self> {
        let draft = Self {
            name: name.into(),
            transport,
        };
        draft.validate()?;
        Ok(draft)
    }

    fn validate(&self) -> Result<()> {
        // Name rules live in `super` so the CLI rejects the same names identically.
        super::validate_server_name(&self.name)?;
        match &self.transport {
            McpDraftTransport::Stdio { command, env, .. } => {
                if command.trim().is_empty() {
                    bail!("a stdio server needs a non-empty --command");
                }
                for name in env.keys() {
                    if name.is_empty() {
                        bail!("--env needs a non-empty variable name");
                    }
                    if name.chars().any(|c| c == '=' || c.is_control()) {
                        bail!("--env name '{name}' must not contain '=' or control characters");
                    }
                }
            }
            McpDraftTransport::Remote {
                url,
                headers,
                oauth: _,
            } => {
                validate_remote_url(url)?;
                for (name, value) in headers {
                    if HeaderName::try_from(name.as_str()).is_err() {
                        bail!("--header '{name}' is not a valid HTTP header name");
                    }
                    // Never echo the value: headers are a common place for
                    // secrets such as `Authorization: Bearer …`.
                    if HeaderValue::try_from(value.as_str()).is_err() {
                        bail!("--header '{name}' has a value that is not a valid HTTP header");
                    }
                }
            }
        }
        Ok(())
    }

    /// The transport as a diagnostic descriptor, for CLI output.
    #[must_use]
    pub fn transport_kind(&self) -> McpTransportKind {
        match &self.transport {
            McpDraftTransport::Stdio { command, .. } => McpTransportKind::Stdio {
                command: command.clone(),
            },
            McpDraftTransport::Remote { url, oauth, .. } => McpTransportKind::Remote {
                url: url.clone(),
                oauth: *oauth,
            },
        }
    }

    /// The JSON object stored under `mcpServers.<name>`.
    ///
    /// Empty collections are omitted so the written entry stays close to what a
    /// user would have typed by hand.
    fn to_json(&self) -> Value {
        let mut entry = Map::new();
        match &self.transport {
            McpDraftTransport::Stdio { command, args, env } => {
                entry.insert("command".to_owned(), Value::String(command.clone()));
                if !args.is_empty() {
                    entry.insert("args".to_owned(), json!(args));
                }
                if !env.is_empty() {
                    entry.insert("env".to_owned(), json!(env));
                }
            }
            McpDraftTransport::Remote {
                url,
                headers,
                oauth,
            } => {
                // `type` is optional (a `url` alone already means remote) but
                // writing it makes the entry self-describing in the file.
                entry.insert("type".to_owned(), Value::String("http".to_owned()));
                entry.insert("url".to_owned(), Value::String(url.clone()));
                if !headers.is_empty() {
                    entry.insert("headers".to_owned(), json!(headers));
                }
                if *oauth {
                    // Minimal declaration: dynamic client registration, no
                    // scopes, ephemeral loopback port.
                    entry.insert("auth".to_owned(), json!({ "type": "oauth" }));
                }
            }
        }
        Value::Object(entry)
    }
}

/// Outcome of a successful [`add_mcp_server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddedMcpServer {
    /// The file that was written.
    pub path: PathBuf,
    /// Whether an entry with the same name was replaced (`--force`).
    pub replaced: bool,
}

/// Adds, or with `force` replaces, one server in the `mcp.json` for `scope`.
///
/// A missing file is created. An existing file keeps every key Tact does not
/// model, including keys of unrelated servers. A file that cannot be parsed is
/// an error rather than a clobber target: it is user-authored data, and losing
/// it would be worse than a failed command.
pub fn add_mcp_server(
    workdir: &Path,
    scope: McpConfigScope,
    draft: &McpServerDraft,
    force: bool,
) -> Result<AddedMcpServer> {
    draft.validate()?;
    let path = scope.path(workdir)?;
    tracing::debug!(
        mcp_server = %draft.name,
        path = %path.display(),
        scope = ?scope,
        transport = %draft.transport_kind(),
        "adding MCP server to config"
    );

    let mut root = read_document(&path)?;
    let replaced = {
        let object = root
            .as_object_mut()
            .with_context(|| format!("{}: expected a JSON object", path.display()))?;
        let servers = object
            .entry("mcpServers")
            .or_insert_with(|| Value::Object(Map::new()));
        let servers = servers
            .as_object_mut()
            .with_context(|| format!("{}: `mcpServers` is not an object", path.display()))?;

        let replaced = servers.contains_key(&draft.name);
        if replaced && !force {
            bail!(
                "MCP server '{}' is already declared in {} — pass --force to replace it",
                draft.name,
                path.display()
            );
        }
        servers.insert(draft.name.clone(), draft.to_json());
        replaced
    };

    write_document(&path, &root)?;

    if replaced {
        tracing::warn!(
            mcp_server = %draft.name,
            path = %path.display(),
            "replaced an existing MCP server declaration"
        );
    }
    tracing::info!(
        mcp_server = %draft.name,
        path = %path.display(),
        scope = ?scope,
        replaced,
        "wrote MCP server declaration"
    );

    Ok(AddedMcpServer { path, replaced })
}

/// Outcome of a successful [`remove_mcp_server`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedMcpServer {
    /// The file that was rewritten.
    pub path: PathBuf,
    /// The entry as it was stored, so a caller can report what was deleted.
    pub removed: Value,
}

/// Removes one server declaration from the `mcp.json` for `scope`.
///
/// An unknown name is an error rather than a silent no-op: the caller (a CLI
/// or an agent) needs to know the config did not change, and a name that is
/// declared in the *other* scope or by a plugin would otherwise be reported as
/// a successful removal that never happened.
///
/// `mcpServers` is left in place (empty) when its last entry goes, so the edit
/// stays predictable and every other key in the document is preserved. Stored
/// OAuth credentials are deliberately **not** deleted — that is `logout`'s job,
/// and a re-added server should keep working.
pub fn remove_mcp_server(
    workdir: &Path,
    scope: McpConfigScope,
    server_name: &str,
) -> Result<RemovedMcpServer> {
    super::validate_server_name(server_name)?;
    let path = scope.path(workdir)?;
    tracing::debug!(
        mcp_server = %server_name,
        path = %path.display(),
        scope = ?scope,
        "removing MCP server from config"
    );

    if !path.is_file() {
        bail!(
            "no MCP config at {} — '{server_name}' is not declared there",
            path.display()
        );
    }
    let mut root = read_document(&path)?;
    let removed = {
        let object = root
            .as_object_mut()
            .with_context(|| format!("{}: expected a JSON object", path.display()))?;
        let servers = object
            .get_mut("mcpServers")
            .and_then(Value::as_object_mut)
            .with_context(|| {
                format!(
                    "{}: no `mcpServers` object — '{server_name}' is not declared there",
                    path.display()
                )
            })?;
        servers.remove(server_name).with_context(|| {
            format!(
                "MCP server '{server_name}' is not declared in {}",
                path.display()
            )
        })?
    };

    write_document(&path, &root)?;

    tracing::info!(
        mcp_server = %server_name,
        path = %path.display(),
        scope = ?scope,
        "removed MCP server declaration"
    );

    Ok(RemovedMcpServer { path, removed })
}

/// Reads the document at `path`; a missing or empty file becomes `{}`.
fn read_document(path: &Path) -> Result<Value> {
    if !path.is_file() {
        return Ok(Value::Object(Map::new()));
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read MCP config {}", path.display()))?;
    // A UTF-8 BOM is invisible to the user but `serde_json` rejects it, which
    // would report an otherwise valid file as unparseable.
    let raw = raw.strip_prefix('\u{feff}').unwrap_or(&raw);
    if raw.trim().is_empty() {
        return Ok(Value::Object(Map::new()));
    }
    let parsed: Value = serde_json::from_str(raw).with_context(|| {
        format!(
            "failed to parse MCP config {} — fix or remove it before adding a server",
            path.display()
        )
    })?;
    if !parsed.is_object() {
        bail!(
            "{}: expected a JSON object at the top level",
            path.display()
        );
    }
    Ok(parsed)
}

/// Writes `document` to `path` via a sibling temp file and a rename.
///
/// The rename keeps the previous file intact if the write fails halfway, so a
/// failed `mcp add` can never leave a truncated, unparseable `mcp.json`.
///
/// The original file's permissions are carried over to the replacement: an
/// `mcp.json` may hold `headers` / `env` secrets, and a user who hardened it
/// with `chmod 600` must not have that silently widened by a rename onto a
/// umask-default inode.
fn write_document(path: &Path, document: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let mut text =
        serde_json::to_string_pretty(document).context("failed to serialize MCP config")?;
    text.push('\n');

    // Unique per call: a fixed sibling name would let two concurrent `mcp add`
    // runs truncate each other's temp file.
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let temp = path.with_extension(format!("json.{}.{unique}.tmp", std::process::id()));
    fs::write(&temp, text).with_context(|| format!("failed to write {}", temp.display()))?;
    let previous_permissions = fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions());
    if let Some(permissions) = previous_permissions
        && let Err(error) = fs::set_permissions(&temp, permissions)
    {
        tracing::warn!(
            path = %temp.display(),
            error = %error,
            "failed to preserve MCP config permissions"
        );
    }
    if let Err(error) = fs::rename(&temp, path) {
        // Never leave a stray temp sibling behind on failure.
        let _ = fs::remove_file(&temp);
        return Err(error).with_context(|| {
            format!(
                "failed to move {} into place at {}",
                temp.display(),
                path.display()
            )
        });
    }
    Ok(())
}

/// Rejects URLs Tact could not route.
///
/// Plain `http://` is accepted (a localhost test server is legitimate) but
/// logged, because a real endpoint should not be.
fn validate_remote_url(url: &str) -> Result<()> {
    if url.trim() != url || url.is_empty() {
        bail!("a remote server needs a non-empty --url without surrounding whitespace");
    }
    if url.starts_with("http://") {
        // Not an error — a localhost test server may legitimately be plain
        // HTTP — but worth saying out loud, because a real endpoint should not
        // be.
        tracing::warn!(url = %url, "remote MCP url is not HTTPS");
        return Ok(());
    }
    if !url.starts_with("https://") {
        bail!("--url '{url}' must start with http:// or https://");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::mcp::{McpAuthConfig, McpConfigFile, McpTransportConfig};

    fn remote(name: &str, url: &str, oauth: bool) -> McpServerDraft {
        McpServerDraft::new(
            name,
            McpDraftTransport::Remote {
                url: url.to_owned(),
                headers: BTreeMap::new(),
                oauth,
            },
        )
        .unwrap()
    }

    fn stdio(name: &str, command: &str) -> McpServerDraft {
        McpServerDraft::new(
            name,
            McpDraftTransport::Stdio {
                command: command.to_owned(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        )
        .unwrap()
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn creates_a_project_file_for_a_remote_server() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap();

        assert!(
            outcome.path.ends_with(".tact/mcp.json"),
            "{:?}",
            outcome.path
        );
        assert!(!outcome.replaced);
        assert_eq!(
            read(&outcome.path)["mcpServers"]["figma"],
            json!({ "type": "http", "url": "https://mcp.figma.com/mcp" })
        );
    }

    #[test]
    fn writes_oauth_declaration_for_remote_server() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("linear", "https://mcp.linear.app/mcp", true),
            false,
        )
        .unwrap();

        assert_eq!(
            read(&outcome.path)["mcpServers"]["linear"],
            json!({
                "type": "http",
                "url": "https://mcp.linear.app/mcp",
                "auth": { "type": "oauth" }
            })
        );
    }

    #[test]
    fn writes_stdio_entry_with_args_and_env() {
        let dir = tempfile::tempdir().unwrap();
        let draft = McpServerDraft::new(
            "basic-memory",
            McpDraftTransport::Stdio {
                command: "uvx".to_owned(),
                args: vec!["basic-memory".to_owned(), "mcp".to_owned()],
                env: BTreeMap::from([("LOG_LEVEL".to_owned(), "debug".to_owned())]),
            },
        )
        .unwrap();

        let outcome = add_mcp_server(dir.path(), McpConfigScope::Project, &draft, false).unwrap();
        assert_eq!(
            read(&outcome.path)["mcpServers"]["basic-memory"],
            json!({
                "command": "uvx",
                "args": ["basic-memory", "mcp"],
                "env": { "LOG_LEVEL": "debug" }
            })
        );
    }

    #[test]
    fn adding_a_second_server_keeps_unknown_keys_and_the_first_server() {
        // The whole reason this module edits the raw document: a field Tact
        // does not model (here `disabled` and a top-level `note`) must survive.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tact/mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            r#"{"note":"keep me","mcpServers":{"existing":{"command":"/bin/echo","disabled":true}}}"#,
        )
        .unwrap();

        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap();

        let document = read(&path);
        assert_eq!(document["note"], json!("keep me"));
        assert_eq!(
            document["mcpServers"]["existing"],
            json!({ "command": "/bin/echo", "disabled": true })
        );
        assert_eq!(
            document["mcpServers"]["figma"]["url"],
            json!("https://mcp.figma.com/mcp")
        );
    }

    #[test]
    fn remove_deletes_only_that_entry_and_keeps_everything_else() {
        let dir = tempfile::tempdir().unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("keep", "/bin/echo"),
            false,
        )
        .unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("drop", "https://mcp.example.invalid/mcp", false),
            false,
        )
        .unwrap();
        let path = dir.path().join(".tact/mcp.json");
        // A hand-added key outside `mcpServers` must survive a removal.
        let document: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let mut document = document;
        document["note"] = json!("keep me");
        fs::write(&path, serde_json::to_string_pretty(&document).unwrap()).unwrap();

        let outcome = remove_mcp_server(dir.path(), McpConfigScope::Project, "drop").unwrap();
        assert!(outcome.path.ends_with(".tact/mcp.json"));
        assert_eq!(
            outcome.removed["url"],
            json!("https://mcp.example.invalid/mcp")
        );

        let document = read(&path);
        assert_eq!(document["note"], json!("keep me"));
        assert!(document["mcpServers"].get("drop").is_none());
        assert_eq!(
            document["mcpServers"]["keep"]["command"],
            json!("/bin/echo")
        );
    }

    #[test]
    fn removing_the_last_server_leaves_an_empty_but_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("only", "/bin/echo"),
            false,
        )
        .unwrap();

        remove_mcp_server(dir.path(), McpConfigScope::Project, "only").unwrap();

        // The key stays (predictable edit); the loader must read this as an
        // empty configuration rather than a malformed one.
        let file = McpConfigFile::read(&dir.path().join(".tact/mcp.json"))
            .unwrap()
            .unwrap();
        assert!(file.mcp_servers.is_empty());
    }

    #[test]
    fn remove_reports_an_absent_name_instead_of_succeeding_silently() {
        let dir = tempfile::tempdir().unwrap();

        // No file at all.
        let error = remove_mcp_server(dir.path(), McpConfigScope::Project, "ghost").unwrap_err();
        assert!(format!("{error:#}").contains("no MCP config"), "{error:#}");

        // File exists but has no such key.
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("present", "/bin/echo"),
            false,
        )
        .unwrap();
        let error = remove_mcp_server(dir.path(), McpConfigScope::Project, "ghost").unwrap_err();
        assert!(
            format!("{error:#}").contains("is not declared"),
            "{error:#}"
        );

        // And the existing entry is untouched.
        let file = McpConfigFile::read(&dir.path().join(".tact/mcp.json"))
            .unwrap()
            .unwrap();
        assert!(file.mcp_servers.contains_key("present"));
    }

    #[test]
    fn an_unsafe_server_name_is_rejected_by_both_add_and_remove() {
        let dir = tempfile::tempdir().unwrap();

        // A name that would escape the config directory, or the OAuth
        // credential directory, must never be written.
        for name in ["../../evil", "sub/dir", "..", "."] {
            let error = McpServerDraft::new(
                name,
                McpDraftTransport::Stdio {
                    command: "/bin/echo".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            )
            .unwrap_err();
            assert!(format!("{error:#}").contains("name"), "{name}: {error:#}");

            let error = remove_mcp_server(dir.path(), McpConfigScope::Project, name).unwrap_err();
            assert!(format!("{error:#}").contains("name"), "{name}: {error:#}");
        }
        assert!(!dir.path().join(".tact/mcp.json").exists());
    }

    #[test]
    fn refuses_to_replace_without_force_and_replaces_with_it() {
        let dir = tempfile::tempdir().unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap();

        let error = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://example.invalid/mcp", false),
            false,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("--force"), "{error:#}");

        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://example.invalid/mcp", false),
            true,
        )
        .unwrap();
        assert!(outcome.replaced);
        assert_eq!(
            read(&outcome.path)["mcpServers"]["figma"]["url"],
            json!("https://example.invalid/mcp")
        );
    }

    #[test]
    fn an_unparseable_file_is_never_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tact/mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();

        let error = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap_err();

        assert!(format!("{error:#}").contains("mcp.json"), "{error:#}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn a_flat_mcp_servers_value_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tact/mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, r#"{"mcpServers":[]}"#).unwrap();

        let error = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("not an object"), "{error:#}");
    }

    #[test]
    fn the_written_file_has_no_leftover_temp_sibling() {
        let dir = tempfile::tempdir().unwrap();
        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("local", "/bin/echo"),
            false,
        )
        .unwrap();

        let leftovers: Vec<String> = fs::read_dir(outcome.path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files left behind: {leftovers:?}"
        );
    }

    /// `mcp.json` can hold `headers`/`env` secrets, so a user who hardened it
    /// must not have that mode silently widened by the rewrite.
    #[cfg(unix)]
    #[test]
    fn rewriting_preserves_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("first", "/bin/echo"),
            false,
        )
        .unwrap();
        fs::set_permissions(&outcome.path, fs::Permissions::from_mode(0o600)).unwrap();

        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("second", "/bin/echo"),
            false,
        )
        .unwrap();

        let mode = fs::metadata(&outcome.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "hardened mode was widened to {mode:o}");
        assert_eq!(
            read(&outcome.path)["mcpServers"].as_object().unwrap().len(),
            2
        );
    }

    /// A BOM is invisible to the user but `serde_json` rejects it; it must not
    /// read as an unparseable config.
    #[test]
    fn a_leading_bom_does_not_block_editing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".tact/mcp.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "\u{feff}{\"mcpServers\":{\"existing\":{\"command\":\"/bin/true\"}}}",
        )
        .unwrap();

        let outcome = add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("local", "/bin/echo"),
            false,
        )
        .unwrap();

        let servers = read(&outcome.path)["mcpServers"]
            .as_object()
            .unwrap()
            .clone();
        assert!(servers.contains_key("existing"), "{servers:?}");
        assert!(servers.contains_key("local"), "{servers:?}");
    }

    #[test]
    fn invalid_names_and_transports_are_rejected_before_writing() {
        let dir = tempfile::tempdir().unwrap();

        for (name, transport) in [
            (
                "",
                McpDraftTransport::Stdio {
                    command: "/bin/echo".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            ),
            (
                "has space",
                McpDraftTransport::Stdio {
                    command: "/bin/echo".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            ),
            (
                "trailing ",
                McpDraftTransport::Stdio {
                    command: "/bin/echo".into(),
                    args: Vec::new(),
                    env: BTreeMap::new(),
                },
            ),
        ] {
            let error = McpServerDraft::new(name, transport).unwrap_err();
            assert!(format!("{error:#}").contains("name"), "{error:#}");
        }

        // A whitespace-free draft with a blank command is equally invalid.
        let error = McpServerDraft::new(
            "ok",
            McpDraftTransport::Stdio {
                command: "   ".into(),
                args: Vec::new(),
                env: BTreeMap::new(),
            },
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("--command"), "{error:#}");

        assert!(
            McpServerDraft::new(
                "ok",
                McpDraftTransport::Remote {
                    url: "ftp://example.invalid".into(),
                    headers: BTreeMap::new(),
                    oauth: false,
                },
            )
            .is_err()
        );
        assert!(
            McpServerDraft::new(
                "ok",
                McpDraftTransport::Remote {
                    url: "https://example.invalid/mcp".into(),
                    headers: BTreeMap::from([("bad header".to_owned(), "v".to_owned())]),
                    oauth: false,
                },
            )
            .is_err()
        );
        assert!(
            !dir.path().join(".tact/mcp.json").exists(),
            "a rejected draft must not create a file"
        );
    }

    #[test]
    fn written_entries_load_back_through_the_reader() {
        // The write shape and the read shape are separate code paths: a `type`
        // or `auth` spelling that only satisfies one of them would ship a
        // server that never connects.
        let dir = tempfile::tempdir().unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &stdio("local", "/bin/echo"),
            false,
        )
        .unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("figma", "https://mcp.figma.com/mcp", false),
            false,
        )
        .unwrap();
        add_mcp_server(
            dir.path(),
            McpConfigScope::Project,
            &remote("linear", "https://mcp.linear.app/mcp", true),
            false,
        )
        .unwrap();

        let path = dir.path().join(".tact/mcp.json");
        let file = McpConfigFile::read(&path).unwrap().unwrap();

        let local = file.mcp_servers["local"].to_transport().unwrap();
        assert!(matches!(
            local,
            McpTransportConfig::Stdio(config) if config.command == "/bin/echo"
        ));
        let figma = file.mcp_servers["figma"].to_transport().unwrap();
        assert!(matches!(
            figma,
            McpTransportConfig::Remote(config) if config.url == "https://mcp.figma.com/mcp" && config.auth.is_none()
        ));
        let linear = file.mcp_servers["linear"].to_transport().unwrap();
        assert!(matches!(
            linear,
            McpTransportConfig::Remote(config)
                if config.url == "https://mcp.linear.app/mcp"
                    && matches!(config.auth, Some(McpAuthConfig::Oauth { .. }))
        ));
    }

    #[test]
    fn project_scope_targets_the_workdir_and_user_scope_the_home_dir() {
        // `path()` is pure, so this holds whatever the developer's HOME is.
        assert_eq!(
            McpConfigScope::Project
                .path(Path::new("/tmp/project"))
                .unwrap(),
            Path::new("/tmp/project/.tact/mcp.json")
        );
        if let Some(home) = TactPath::home_mcp_config_path() {
            assert_eq!(
                McpConfigScope::User
                    .path(Path::new("/tmp/project"))
                    .unwrap(),
                home
            );
        }
    }
}
