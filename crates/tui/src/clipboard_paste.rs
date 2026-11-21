//! Turning a clipboard payload into `@` references the input box can hold.
//!
//! A terminal delivers a paste as **text** (`Event::Paste`). A copied file
//! arrives as a `file://…` URI list, but a copied *image* has no text form at
//! all, so before this module a screenshot paste did nothing. Desktop solved
//! the same problem in `crates/desktop/src/ui/prompt_bar.rs::clipboard_image_path`
//! by reading the clipboard itself; this is that idea for a terminal.
//!
//! A reference must survive [`tact::tool::safe_path`], which rejects anything
//! outside the work directory. Clipboard files usually live in `~/Downloads`
//! or `/tmp`, so an outside file is first **copied** into
//! [`TactPath::attachments_dir`] (`.tact/attachments/`, already git-ignored)
//! and the reference points at the copy. The sandbox is not loosened.

use std::path::{Path, PathBuf};

use arboard::Clipboard;
use tact::consts::TactPath;

/// One clipboard read.
///
/// The pasteboard is process-global and each `get_*` is a round trip through
/// the compositor, so the payload kinds are read together rather than each
/// affordance opening its own handle.
pub(crate) struct Snapshot {
    /// Files the clipboard lists.
    pub files: Vec<PathBuf>,
    /// A bitmap (a screenshot). `arboard` decodes it to raw RGBA.
    pub image: Option<(usize, usize, Vec<u8>)>,
    /// Plain text, when the clipboard held neither a file nor an image.
    pub text: Option<String>,
}

/// Read everything the pasteboard can offer, preferring structured payloads.
///
/// Never panics and never returns an error: on a headless or SSH session the
/// clipboard may be unavailable, and a paste then simply falls through to the
/// text the terminal already decoded.
pub(crate) fn snapshot() -> Snapshot {
    let mut snapshot = Snapshot {
        files: Vec::new(),
        image: None,
        text: None,
    };

    // Tests: the pasteboard is process-global and this binary runs its tests in
    // parallel threads, where two of them touching it at once crashes the
    // process (see `crate::clipboard_lock`).
    #[cfg(test)]
    let _clipboard = crate::clipboard_lock::take();

    let Ok(mut clipboard) = Clipboard::new() else {
        return snapshot;
    };

    // `file_list` before `text`: a file-manager copy publishes `text/uri-list`,
    // and reading it as text would leave a `file:///` string in the input box.
    if let Ok(paths) = clipboard.get().file_list() {
        snapshot.files = paths.into_iter().filter(|path| path.is_file()).collect();
    }
    if snapshot.files.is_empty()
        && let Ok(text) = clipboard.get_text()
    {
        snapshot.files = files_in_uri_text(&text);
    }

    if let Ok(image) = clipboard.get_image() {
        snapshot.image = Some((image.width, image.height, image.bytes.into_owned()));
    }

    // Text is the last resort: on some platforms a screenshot also publishes an
    // empty string, and a copied file publishes its name.
    if snapshot.files.is_empty()
        && snapshot.image.is_none()
        && let Ok(text) = clipboard.get_text()
        && !text.trim().is_empty()
    {
        snapshot.text = Some(text);
    }

    snapshot
}

/// The files named by a `text/uri-list`-shaped payload (`file:///tmp/a%20b.png`,
/// one per line).
///
/// Requiring the `file://` scheme is what keeps a pasted *sentence* that happens
/// to mention `/tmp/x.png` as text: only the URI form marks a real file
/// transfer. Entries naming a file that no longer exists are dropped, so a
/// stale clipboard does not insert a reference the model cannot read.
pub(crate) fn files_in_uri_text(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.strip_prefix("file://"))
        .map(uri_path)
        .filter(|path| path.is_file())
        .collect()
}

/// The file a paste names outright, when the **whole** paste is one path.
///
/// File managers and `pwd`-style copies put a bare path on the clipboard, and
/// some terminals forward that as the paste text. Without this the path lands
/// as prose: `build_user_message` only recognises a reference that carries the
/// literal `@`, so the model would read a path string it was never told to open.
///
/// Deliberately strict, because guessing wrong rewrites the user's words:
///
/// - the whole paste must be one line (a paragraph that mentions a path is
///   text — the `file://` rule in [`files_in_uri_text`] is not loosened either),
/// - it must name an existing **file**,
/// - `work_dir` is optional because this is also used to decide whether text is
///   a path at all; when given, a relative path resolves against it.
pub(crate) fn file_in_plain_text(text: &str, work_dir: Option<&Path>) -> Option<PathBuf> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.contains('\n') {
        return None;
    }
    // A quoted shell-style path (`'/tmp/my shot.png'`) is what a shell hands
    // over; the `@` grammar quotes differently, so unwrap it here.
    let unquoted = unwrap_quotes(trimmed);
    // `file://` is handled by the URI rule; do not double-report it.
    if unquoted.starts_with("file://") {
        return None;
    }

    let candidate = PathBuf::from(unquoted);
    let resolved = if candidate.is_absolute() {
        candidate
    } else {
        match work_dir {
            Some(dir) => dir.join(candidate),
            None => candidate,
        }
    };
    resolved.is_file().then_some(resolved)
}

/// Strip one matching pair of quotes (`"…"`, `'…'`, or backticks).
fn unwrap_quotes(text: &str) -> &str {
    let bytes = text.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        if first == last && matches!(first, b'"' | b'\'' | b'`') {
            return &text[1..text.len() - 1];
        }
    }
    text
}

/// The local path from a `file://` URI remainder.
fn uri_path(rest: &str) -> PathBuf {
    let decoded = percent_decode(rest);
    // `file:///C:/x.png` carries a leading slash that is not part of the drive
    // path; `file:///tmp/x` on unix is genuinely absolute.
    let bytes = decoded.as_bytes();
    let drive_prefix = bytes.len() > 3 && bytes[0] == b'/' && bytes[2] == b':';
    PathBuf::from(if drive_prefix {
        &decoded[1..]
    } else {
        &decoded
    })
}

/// Decode `%20`-style escapes. Invalid UTF-8 is replaced, not rejected: the
/// path still has to exist before it is referenced.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            out.push(high << 4 | low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// An `@` reference for one clipboard file, plus whether the file had to be
/// copied into the workspace to make it readable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reference {
    /// The `@…` token to insert into the input box.
    pub text: String,
    /// True when the source lived outside the workspace and was copied into
    /// `.tact/attachments` — the caller reports that, because the file the user
    /// referenced is no longer the file the model will read.
    pub copied: bool,
}

/// The `@` expression for `path`, copying it into `.tact/attachments` first
/// when it lives outside `work_dir`.
///
/// Returns `None` when the file cannot be made referenceable (a directory, an
/// unreadable source); the caller skips it rather than inserting a path the
/// sandbox would reject at read time.
pub(crate) fn reference_for(work_dir: &Path, path: &Path) -> Option<Reference> {
    if !path.is_file() {
        return None;
    }
    let work_dir = work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf());
    let source = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

    let (target, copied) = if source.starts_with(&work_dir) {
        (source, false)
    } else {
        let directory = TactPath::new(&work_dir).attachments_dir();
        std::fs::create_dir_all(&directory).ok()?;
        let destination = unique_destination(&directory, &source)?;
        if std::fs::copy(&source, &destination).is_err() {
            return None;
        }
        (destination, true)
    };

    let relative = target.strip_prefix(&work_dir).ok()?;
    // Forward slashes on every platform: the reference is a `@` token, not a
    // shell argument, and `safe_path` joins either separator.
    let relative: String = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");

    Some(Reference {
        text: quote_reference(&relative),
        copied,
    })
}

/// `@"path with spaces"`, or a bare `@path` when nothing needs quoting.
///
/// The `@` grammar's quoted form cannot contain a `"`, so such a name falls back
/// to the bare form — truncated at the first space, the one case the user has to
/// fix by hand.
fn quote_reference(path: &str) -> String {
    if path.contains('"') {
        return format!("@{path}");
    }
    if path.chars().any(char::is_whitespace) {
        return format!("@\"{path}\"");
    }
    format!("@{path}")
}

/// A destination in `directory` that does not clobber an earlier attachment.
///
/// The original name is kept when free so the reference stays readable, and a
/// repeat paste of the same bytes reuses the existing file rather than piling up
/// `shot-1.png`, `shot-2.png` for one image.
fn unique_destination(directory: &Path, source: &Path) -> Option<PathBuf> {
    let name = source.file_name()?;
    let candidate = directory.join(name);
    if !candidate.exists() {
        return Some(candidate);
    }
    if std::fs::read(source).ok() == std::fs::read(&candidate).ok() {
        return Some(candidate);
    }

    let stem = source.file_stem()?.to_string_lossy().into_owned();
    let extension = source
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();
    (1..1000)
        .map(|index| directory.join(format!("{stem}-{index}{extension}")))
        .find(|path| !path.exists())
}

/// Persist a clipboard bitmap as a PNG under `.tact/attachments` and return the
/// `@` reference for it.
///
/// PNG because [`tact::tool::read_image`] decodes by magic bytes, and PNG is the
/// one format the clipboard decoder hands over losslessly.
pub(crate) fn save_image(
    work_dir: &Path,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Option<Reference> {
    let directory = TactPath::new(work_dir).attachments_dir();
    std::fs::create_dir_all(&directory).ok()?;

    let buffer = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())?;
    let destination = unique_destination(&directory, Path::new("clipboard.png"))?;
    buffer.save(&destination).ok()?;

    reference_for(work_dir, &destination)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{
        file_in_plain_text, files_in_uri_text, percent_decode, quote_reference, reference_for,
        uri_path,
    };

    #[test]
    fn percent_decoding_restores_spaces_and_utf8() {
        assert_eq!(percent_decode("a%20b.png"), "a b.png");
        assert_eq!(percent_decode("caf%C3%A9.png"), "café.png");
        // A lone `%` is literal, not a broken escape that eats the tail.
        assert_eq!(percent_decode("100%.png"), "100%.png");
    }

    #[test]
    fn uri_path_drops_the_leading_slash_before_a_windows_drive_only() {
        assert_eq!(
            uri_path("/tmp/a.png"),
            std::path::PathBuf::from("/tmp/a.png")
        );
        assert_eq!(
            uri_path("/C:/x/a.png"),
            std::path::PathBuf::from("C:/x/a.png")
        );
    }

    #[test]
    fn uri_text_yields_only_file_uris_that_exist() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        fs::write(&file, b"x").unwrap();

        let text = format!(
            "just a sentence\r\nfile://{}\r\nfile:///nope/missing.png\r\n# comment\r\n",
            file.display()
        );
        assert_eq!(files_in_uri_text(&text), vec![file]);
    }

    #[test]
    fn a_plain_path_is_recognised_only_when_it_names_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        fs::write(&file, b"x").unwrap();

        assert_eq!(
            file_in_plain_text(&file.display().to_string(), None),
            Some(file.clone())
        );
        assert_eq!(
            file_in_plain_text(&format!("'{}'", file.display()), None),
            Some(file.clone()),
            "a shell-quoted path is what a drag-and-drop hands over"
        );
        assert_eq!(
            file_in_plain_text(&file.display().to_string(), None).map(|p| p.is_file()),
            Some(true),
            "the reference must point at a readable file"
        );
    }

    #[test]
    fn multi_line_text_is_never_a_bare_path() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        fs::write(&file, b"x").unwrap();

        let text = format!(
            "{}
and some notes",
            file.display()
        );
        assert_eq!(
            file_in_plain_text(&text, None),
            None,
            "a path plus commentary is prose, not a reference"
        );
    }

    #[test]
    fn a_plain_path_that_no_longer_exists_is_not_a_reference() {
        assert_eq!(file_in_plain_text("/nope/deleted.png", None), None);
        assert_eq!(file_in_plain_text("", None), None);
    }

    #[test]
    fn prose_mentioning_a_path_stays_text() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("shot.png");
        fs::write(&file, b"x").unwrap();

        let text = format!("look at {}", file.display());
        assert!(files_in_uri_text(&text).is_empty());
    }

    #[test]
    fn spaces_are_quoted_so_the_at_grammar_keeps_the_whole_path() {
        assert_eq!(quote_reference("a.png"), "@a.png");
        assert_eq!(quote_reference("my shot.png"), "@\"my shot.png\"");
    }

    #[test]
    fn a_file_outside_the_workspace_is_copied_in_and_referenced_relatively() {
        let workspace = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let source = elsewhere.path().join("shot.png");
        fs::write(&source, b"pixels").unwrap();

        let reference = reference_for(workspace.path(), &source).unwrap();

        assert_eq!(reference.text, "@.tact/attachments/shot.png");
        assert!(reference.copied, "an outside file must report the copy");
        assert_eq!(
            fs::read(workspace.path().join(".tact/attachments/shot.png")).unwrap(),
            b"pixels"
        );
    }

    #[test]
    fn a_file_inside_the_workspace_is_referenced_in_place() {
        let workspace = tempfile::tempdir().unwrap();
        let source = workspace.path().join("notes.md");
        fs::write(&source, b"hi").unwrap();

        let reference = reference_for(workspace.path(), &source).unwrap();

        assert_eq!(reference.text, "@notes.md");
        assert!(!reference.copied, "no copy happened, so no notice is due");
        assert!(
            !workspace.path().join(".tact/attachments").exists(),
            "an in-workspace file must not be duplicated"
        );
    }

    #[test]
    fn repasting_the_same_file_reuses_the_attachment() {
        let workspace = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let source = elsewhere.path().join("shot.png");
        fs::write(&source, b"pixels").unwrap();

        let first = reference_for(workspace.path(), &source).unwrap();
        let second = reference_for(workspace.path(), &source).unwrap();

        assert_eq!(first.text, second.text);
        assert_eq!(
            fs::read_dir(workspace.path().join(".tact/attachments"))
                .unwrap()
                .count(),
            1,
            "a repeat paste must not pile up copies"
        );
    }

    #[test]
    fn a_different_file_with_the_same_name_gets_its_own_copy() {
        let workspace = tempfile::tempdir().unwrap();
        let first_source = tempfile::tempdir().unwrap();
        let second_source = tempfile::tempdir().unwrap();
        let first = first_source.path().join("shot.png");
        let second = second_source.path().join("shot.png");
        fs::write(&first, b"one").unwrap();
        fs::write(&second, b"two").unwrap();

        let first = reference_for(workspace.path(), &first).unwrap();
        let second = reference_for(workspace.path(), &second).unwrap();

        assert_eq!(first.text, "@.tact/attachments/shot.png");
        assert_eq!(
            second.text, "@.tact/attachments/shot-1.png",
            "different bytes must not overwrite the earlier attachment"
        );
        assert_eq!(
            fs::read(workspace.path().join(".tact/attachments/shot.png")).unwrap(),
            b"one"
        );
        assert_eq!(
            fs::read(workspace.path().join(".tact/attachments/shot-1.png")).unwrap(),
            b"two"
        );
    }

    #[test]
    fn a_directory_is_not_referenceable() {
        let workspace = tempfile::tempdir().unwrap();
        let directory = workspace.path().join("sub");
        fs::create_dir(&directory).unwrap();

        assert!(
            reference_for(workspace.path(), &directory).is_none(),
            "`@` names files, and `safe_path` would fail on a directory"
        );
    }

    #[test]
    fn a_reference_resolves_through_the_workspace_guard() {
        // The point of copying an outside file in: the reference must pass the
        // same validation the read tools apply, or the model cannot read it.
        let workspace = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let source = elsewhere.path().join("my shot.png");
        fs::write(&source, b"pixels").unwrap();

        let reference = reference_for(workspace.path(), &source).unwrap();
        let relative = reference.text.trim_start_matches('@').trim_matches('"');

        assert!(
            tact::tool::safe_path(workspace.path(), relative).is_ok(),
            "{} must resolve inside the workspace",
            reference.text
        );
    }
}
