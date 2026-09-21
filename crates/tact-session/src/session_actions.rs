//! Session actions shared by both front ends: rename, duplicate, archive,
//! pin, and reveal in the file manager.
//!
//! The store owns the facts -- a session's name, its archive flag, the copy --
//! so a front end calls one of these and redraws its list from
//! [`crate::sessions::recent`]. Everything here is scoped to a workspace's own
//! `.tact/tact.db`, the same store the list is read from, so an action can never
//! touch another workspace's session.
//!
//! Three of these are deliberately conservative. **Archiving sets a flag and
//! never deletes**: the session, its messages and its child sessions all
//! survive, so clearing the flag restores the row exactly as it was. **Pinning
//! only sets a marker**: the session keeps its place in the store and simply
//! sorts ahead of the unpinned rows. **Duplicating copies
//! the conversation, not the provider state**: the copy replays its messages on
//! its first turn instead of resuming a request/response chain that belonged to
//! the original.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::{process::Command, thread, time::Duration};

use tact::consts::TactPath;

use crate::sessions::{session_title, short_id};

/// Programs that can open a directory, in the order the desktop client tries
/// them. `gio` needs its subcommand spelled out; the others take the path alone.
const LAUNCHERS: [(&str, &[&str]); 6] = [
    ("xdg-open", &[]),
    ("gio", &["open"]),
    ("nautilus", &[]),
    ("dolphin", &[]),
    ("open", &[]),
    ("explorer", &[]),
];

/// Name a session, or clear the name when `title` is empty.
///
/// An unnamed session falls back to its derived label, which is why clearing is
/// spelled as an empty title rather than a separate call.
pub fn rename(workdir: &Path, session_id: &str, title: &str) -> anyhow::Result<()> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    runtime()?.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        store.rename_session(session_id, title).await
    })
}

/// Set or clear a session's archived flag.
///
/// This never deletes: the row keeps its messages and stays listable, so
/// `set_archived(.., false)` gives the session back.
pub fn set_archived(workdir: &Path, session_id: &str, archived: bool) -> anyhow::Result<()> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    runtime()?.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        store.archive_session(session_id, archived).await
    })
}

/// Set or clear a session's pinned flag.
///
/// Pinning is list order, not state: the row, its messages, and its children
/// are untouched, and unpinning returns the session to the ordinary date order.
pub fn set_pinned(workdir: &Path, session_id: &str, pinned: bool) -> anyhow::Result<()> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    runtime()?.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        store.pin_session(session_id, pinned).await
    })
}

/// Copy a session's conversation into a new one and return the new id.
///
/// The copy holds the source's messages and nothing else: no provider state (so
/// its first turn replays the conversation rather than resuming the original's
/// chain) and no recorded token usage (that spend was the original's). It is
/// named after the source with a `(copy)` suffix, because a duplicate that reads
/// exactly like the session it came from is indistinguishable in a list.
pub fn duplicate(workdir: &Path, session_id: &str) -> anyhow::Result<String> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    let root_dir = tact_path.workdir().display().to_string();
    let new_id = uuid::Uuid::new_v4().to_string();

    runtime()?.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        let name = copy_name(&store, &root_dir, session_id).await?;
        store.duplicate_session(session_id, &new_id).await?;
        store.rename_session(&new_id, &name).await?;
        Ok(new_id)
    })
}

/// Open `workdir` in the desktop's file manager.
///
/// Failure is reported rather than swallowed: a "Reveal in filesystem" that
/// silently does nothing is worse than one that says no launcher was found.
pub fn reveal(workdir: &Path) -> anyhow::Result<()> {
    let dir = TactPath::new(workdir.to_path_buf()).workdir().to_path_buf();
    if !dir.is_dir() {
        anyhow::bail!("cannot reveal {}: not a directory", dir.display());
    }
    launch_path(&dir, "reveal")
}

/// Reveal `path` in the desktop's file manager.
///
/// A file is revealed through its parent: the launchers used here open the
/// target they are handed, and handing them a directory keeps "Reveal" distinct
/// from "Open in editor". The caller gets the path in the error or success text
/// so a press never silently opens the wrong location.
pub fn reveal_path(path: &Path) -> anyhow::Result<()> {
    let target = reveal_target(path);
    if !target.exists() {
        anyhow::bail!("cannot reveal {}: it does not exist", path.display());
    }
    launch_path(&target, "reveal")
}

fn reveal_target(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().unwrap_or(path).to_path_buf()
    }
}

/// Open `path` with the platform's default application.
///
/// This is the file-manager-neutral counterpart to [`reveal_path`]. On Linux
/// `xdg-open` is typically the desktop's MIME opener; on macOS `open` uses the
/// default application. The function reports launcher failures rather than
/// pretending a press worked.
pub fn open_path(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("cannot open {}: it does not exist", path.display());
    }
    launch_path(path, "open")
}

/// Open an `http` or `https` URL in the desktop's default browser.
///
/// The URL is handed to the same launcher chain [`open_path`] uses, but it is
/// never canonicalized: a URL is not a filesystem path, and `Path::exists` on
/// one would both be meaningless and reject every valid link.
pub fn open_url(url: &str) -> anyhow::Result<()> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        anyhow::bail!("cannot open an empty URL");
    }
    let lowered = trimmed.to_ascii_lowercase();
    if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
        anyhow::bail!("only http and https URLs can be opened: {trimmed}");
    }
    let url = OsString::from(trimmed);
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let mut tried = Vec::new();
    for (program, subcommand) in LAUNCHERS {
        let Some(launcher) = find_on_path(program, &path_env) else {
            tried.push(program);
            continue;
        };
        let mut args: Vec<OsString> = subcommand.iter().map(OsString::from).collect();
        args.push(url.clone());
        let mut child = Command::new(&launcher)
            .args(&args)
            .spawn()
            .map_err(|error| anyhow::anyhow!("failed to launch {}: {error}", launcher.display()))?;
        // Same grace period as `launch_path`: a launcher that is present but
        // has no browser to hand the URL to must be reported, not swallowed.
        thread::sleep(Duration::from_millis(150));
        match child.try_wait() {
            Ok(Some(status)) if !status.success() => {
                anyhow::bail!(
                    "{} exited with {} while trying to open {trimmed}",
                    launcher.display(),
                    status
                );
            }
            // `Err` is a launcher that is still starting; treat it as handed
            // over and reap it in the background, exactly as `launch_path`
            // does for a file.
            Ok(_) | Err(_) => {
                thread::spawn(move || {
                    let _ = child.wait();
                });
                return Ok(());
            }
        }
    }
    anyhow::bail!("no URL launcher on PATH (tried {})", tried.join(", "))
}

/// Stage one changed path in the workspace's Git index.
///
/// The Diff pane records changes at tool-call time, but a stage decision is a
/// Git operation on the working tree. Resolve the recorded path the same way
/// the diff reader does so a repository-root path still points at the right
/// file when the session workspace is a subdirectory.
pub fn stage_path(workdir: &Path, path: &str) -> anyhow::Result<()> {
    let target = resolve_git_path(workdir, path);
    let output = Command::new("git")
        .arg("-C")
        .arg(workdir)
        .args(["add", "--"])
        .arg(&target)
        .output()
        .map_err(|error| anyhow::anyhow!("failed to run git add: {error}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git add {} failed: {}",
            target.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn resolve_git_path(workdir: &Path, path: &str) -> PathBuf {
    let recorded = Path::new(path);
    if recorded.is_absolute() {
        return recorded.to_path_buf();
    }
    let resolved = workdir.join(recorded);
    if resolved.exists() {
        resolved
    } else {
        PathBuf::from(format!(":(top){path}"))
    }
}

fn launch_path(path: &Path, verb: &str) -> anyhow::Result<()> {
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let (launcher, args) = reveal_command(path, &path_env)?;
    let mut child = Command::new(&launcher)
        .args(&args)
        .spawn()
        .map_err(|error| anyhow::anyhow!("failed to launch {}: {error}", launcher.display()))?;
    // Most launchers exit quickly, and a launcher that is present but cannot
    // open the desktop (for example `xdg-open` with no default handler) would
    // otherwise be reported as success. Give it a short grace period, then keep
    // reaping in the background so repeated reveals do not accumulate zombies.
    thread::sleep(Duration::from_millis(150));
    match child.try_wait() {
        Ok(Some(status)) if !status.success() => anyhow::bail!(
            "{} exited with {} while trying to {verb} {}",
            launcher.display(),
            status,
            path.display()
        ),
        Ok(_) => {
            thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(())
        }
        Err(error) => anyhow::bail!(
            "failed to check {} after launching it: {error}",
            launcher.display()
        ),
    }
}

/// The launcher to run, and the arguments that open `dir`.
///
/// Resolution is by hand rather than by shelling out to `which`: the desktop
/// client has to name the program it ran in its error, and a missing launcher
/// must be a reported failure instead of a silent no-op.
fn reveal_command(
    dir: &Path,
    path_env: &std::ffi::OsStr,
) -> anyhow::Result<(PathBuf, Vec<OsString>)> {
    let mut tried = Vec::new();
    for (program, subcommand) in LAUNCHERS {
        if let Some(found) = find_on_path(program, path_env) {
            let mut args: Vec<OsString> = subcommand.iter().map(OsString::from).collect();
            args.push(dir.as_os_str().to_os_string());
            return Ok((found, args));
        }
        tried.push(program);
    }
    anyhow::bail!(
        "no file-manager launcher on PATH (tried {})",
        tried.join(", ")
    )
}

/// The first directory on `PATH` that holds an executable named `program`.
fn find_on_path(program: &str, path_env: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_env)
        .filter(|dir| !dir.as_os_str().is_empty())
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

/// The name to give a duplicate of `session_id`.
///
/// Prefers the name the user gave the session, then the derived opening-message
/// label, then the short id -- the same order a front end labels the row with,
/// so the copy reads as a copy of the row the user pressed.
async fn copy_name(
    store: &tact::store::DynSessionStore,
    root_dir: &str,
    session_id: &str,
) -> anyhow::Result<String> {
    let sessions = store.list_sessions(Some(root_dir)).await?;
    let base = sessions
        .iter()
        .find(|session| session.id == session_id)
        .and_then(|session| {
            session
                .title
                .clone()
                .or_else(|| session.first_user_text.as_deref().and_then(session_title))
        })
        .unwrap_or_else(|| short_id(session_id).to_string());
    Ok(format!("{base} (copy)"))
}

fn runtime() -> anyhow::Result<tokio::runtime::Runtime> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(runtime)
}

#[cfg(test)]
mod tests {
    use tact_llm::{Message, ProviderConversationState, ResponsesConversationState, Role};

    use super::*;
    use crate::{history::history, sessions::recent};

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tact_session_actions_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp workspace");
        dir
    }

    /// Seed a workspace store with one session and the user messages it owns.
    fn seed(workspace: &Path, session_id: &str, texts: &[&str]) {
        let tact_path = TactPath::new(workspace.to_path_buf());
        let root_dir = workspace.display().to_string();
        runtime().expect("runtime").block_on(async {
            let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                .await
                .expect("store");
            store
                .ensure_session_row(session_id, &root_dir, "")
                .await
                .expect("row");
            for (ordinal, text) in texts.iter().enumerate() {
                let message = Message::new_text(Role::User, *text);
                store
                    .append_message(session_id, message.role, &message.content, ordinal as i64)
                    .await
                    .expect("append");
            }
        });
    }

    /// Plant the two pieces of per-session provider bookkeeping a real turn
    /// leaves behind: a Responses input baseline and a recorded usage row.
    fn plant_provider_state(workspace: &Path, session_id: &str) {
        let tact_path = TactPath::new(workspace.to_path_buf());
        runtime().expect("runtime").block_on(async {
            let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                .await
                .expect("store");
            let messages = store.load_session(session_id).await.expect("messages");
            let state = ProviderConversationState::OpenAiResponses(ResponsesConversationState {
                version: 1,
                provider: "openai".to_string(),
                base_url: "https://api.openai.com/v1".to_string(),
                model: "gpt-5".to_string(),
                input_items: vec![serde_json::json!({"type": "message"})],
                compaction_id: None,
                is_compacted: false,
                logical_message_count: messages.len(),
                logical_context_hash: "hash".to_string(),
            });
            store
                .replace_session_messages_and_provider_state(session_id, &messages, Some(&state))
                .await
                .expect("provider state");
            store
                .record_token_usage(
                    session_id,
                    "chat",
                    None,
                    0,
                    0,
                    Some(b"{\"model\":\"gpt-5\"}"),
                )
                .await
                .expect("usage row");
        });
    }

    fn provider_state(workspace: &Path, session_id: &str) -> Option<ProviderConversationState> {
        let tact_path = TactPath::new(workspace.to_path_buf());
        runtime().expect("runtime").block_on(async {
            let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                .await
                .expect("store");
            store
                .load_provider_state(session_id)
                .await
                .expect("provider state")
        })
    }

    fn latest_request_body(workspace: &Path, session_id: &str) -> Option<Vec<u8>> {
        let tact_path = TactPath::new(workspace.to_path_buf());
        runtime().expect("runtime").block_on(async {
            let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                .await
                .expect("store");
            store
                .load_latest_request_body(session_id)
                .await
                .expect("request body")
        })
    }

    #[test]
    fn renaming_names_the_session_and_clearing_it_restores_the_derived_label() {
        let workspace = temp_workspace();
        let session = "11111111-aaaa";
        seed(&workspace, session, &["Build the desktop shell"]);

        let before = recent(&workspace).expect("recent");
        assert_eq!(before[0].name, None);
        assert_eq!(
            before[0].title.as_deref(),
            Some("Build the desktop shell"),
            "the opening message is the label until the user names the session"
        );

        rename(&workspace, session, "  Desktop client design  ").expect("rename");
        let named = recent(&workspace).expect("recent");
        assert_eq!(
            named[0].name.as_deref(),
            Some("Desktop client design"),
            "the user's name is stored trimmed"
        );
        assert_eq!(
            named[0].title.as_deref(),
            Some("Build the desktop shell"),
            "the derived label stays readable behind the name"
        );

        rename(&workspace, session, "   \n ").expect("clear");
        let cleared = recent(&workspace).expect("recent");
        assert_eq!(
            cleared[0].name, None,
            "blanking the name restores the label"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn archiving_keeps_the_session_and_its_transcript() {
        let workspace = temp_workspace();
        let session = "22222222-bbbb";
        seed(&workspace, session, &["check the build", "and the tests"]);

        set_archived(&workspace, session, true).expect("archive");
        let archived = recent(&workspace).expect("recent");
        assert_eq!(archived.len(), 1, "an archived session stays listable");
        assert!(archived[0].archived);
        assert_eq!(archived[0].message_count, 2, "archiving is not a delete");
        assert_eq!(
            history(&workspace, session).expect("history").len(),
            2,
            "the transcript survives archiving"
        );

        set_archived(&workspace, session, false).expect("unarchive");
        let restored = recent(&workspace).expect("recent");
        assert!(!restored[0].archived, "clearing the flag restores the row");
        assert_eq!(restored[0].message_count, 2);

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn a_duplicate_copies_the_conversation_but_not_the_provider_state() {
        let workspace = temp_workspace();
        let session = "33333333-cccc";
        seed(&workspace, session, &["check the build"]);
        plant_provider_state(&workspace, session);

        let copy = duplicate(&workspace, session).expect("duplicate");
        assert_ne!(copy, session, "the copy gets its own id");

        assert_eq!(
            history(&workspace, &copy).expect("copy history"),
            history(&workspace, session).expect("source history"),
            "the copy holds the same conversation"
        );

        let sessions = recent(&workspace).expect("recent");
        assert_eq!(sessions.len(), 2, "both rows are listable: {sessions:?}");
        let row = sessions
            .iter()
            .find(|candidate| candidate.id == copy)
            .expect("the copy is listed");
        assert_eq!(
            row.name.as_deref(),
            Some("check the build (copy)"),
            "a duplicate is named after the row it came from"
        );
        assert!(!row.archived, "a fresh copy is not archived");

        assert!(
            provider_state(&workspace, session).is_some(),
            "the original keeps its provider baseline"
        );
        assert!(
            provider_state(&workspace, &copy).is_none(),
            "the copy starts from its messages, not the original's chain"
        );
        assert!(
            latest_request_body(&workspace, session).is_some(),
            "the original keeps its recorded usage"
        );
        assert!(
            latest_request_body(&workspace, &copy).is_none(),
            "the copy does not inherit the original's spend"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn rename_and_archive_reject_an_unknown_session() {
        let workspace = temp_workspace();
        let session = "99999999-zzzz";

        let rename_error = rename(&workspace, session, "Nowhere")
            .expect_err("an unknown session cannot be renamed")
            .to_string();
        assert!(rename_error.contains(session), "{rename_error}");

        let archive_error = set_archived(&workspace, session, true)
            .expect_err("an unknown session cannot be archived")
            .to_string();
        assert!(archive_error.contains(session), "{archive_error}");

        let duplicate_error = duplicate(&workspace, session)
            .expect_err("an unknown session cannot be copied")
            .to_string();
        assert!(duplicate_error.contains(session), "{duplicate_error}");

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn reveal_picks_the_first_launcher_on_path_and_reports_when_there_is_none() {
        let workspace = temp_workspace();
        let empty = workspace.join("empty");
        std::fs::create_dir_all(&empty).expect("empty dir");
        // `xdg-open` is missing here, so the second launcher with its `open`
        // subcommand is the one that has to be picked.
        let gio_dir = workspace.join("gio-bin");
        std::fs::create_dir_all(&gio_dir).expect("gio dir");
        let gio = gio_dir.join("gio");
        std::fs::write(&gio, b"").expect("fake launcher");

        let target = Path::new("/tmp/reveal-target");
        let path_env = std::env::join_paths([empty.as_path(), gio_dir.as_path()]).expect("PATH");
        let (launcher, args) = reveal_command(target, &path_env).expect("launcher");
        assert_eq!(launcher, gio);
        assert_eq!(
            args,
            vec![OsString::from("open"), target.as_os_str().to_os_string()]
        );

        let bare = std::env::join_paths([empty.as_path()]).expect("PATH");
        let error = reveal_command(target, &bare)
            .expect_err("no launcher is a reported failure")
            .to_string();
        assert!(
            error.contains("xdg-open") && error.contains("dolphin") && error.contains("explorer"),
            "the error names what was tried: {error}"
        );

        let missing = workspace.join("not-a-directory");
        assert!(
            reveal(&missing).is_err(),
            "revealing a path that is not a directory fails instead of opening the parent"
        );
        let file = workspace.join("reviewed.txt");
        std::fs::write(&file, "review\n").expect("file");
        assert_eq!(
            reveal_target(&file),
            workspace,
            "revealing a file opens its containing directory"
        );
        assert_eq!(
            reveal_target(&workspace),
            workspace,
            "revealing a directory keeps the directory itself"
        );
        assert!(
            open_path(&missing).is_err(),
            "opening a missing path is a reported failure"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn stage_path_stages_a_repository_root_path_from_a_subdirectory() {
        let repo = temp_workspace();
        if std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["init", "--quiet"])
            .status()
            .map(|status| !status.success())
            .unwrap_or(true)
        {
            let _ = std::fs::remove_dir_all(repo);
            return;
        }
        let nested = repo.join("src/nested");
        std::fs::create_dir_all(&nested).expect("nested workspace");
        std::fs::write(repo.join("src/lib.rs"), "pub fn lib() {}\n").expect("file");

        stage_path(&nested, "src/lib.rs").expect("repository-root path stages");

        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(["diff", "--cached", "--name-only"])
            .output()
            .expect("git diff");
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "src/lib.rs",
            "the nested workspace path resolves through the repository root"
        );

        let _ = std::fs::remove_dir_all(repo);
    }
}
