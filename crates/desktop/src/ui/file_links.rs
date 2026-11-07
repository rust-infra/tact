//! Display-only file links for assistant Markdown.
//!
//! Absolute paths inside inline code stay absolute in the underlying message.
//! The transcript shows them relative to the project (or home), and clicking
//! opens the real file with the system's default application.

use std::path::{Path, PathBuf};

use url::Url;

/// Rewrites absolute file paths in inline code into compact `file://` links.
///
/// Fenced code blocks and prose are left untouched. A path must exist to be
/// linked so stale or hypothetical paths keep their original text.
pub fn linkify_paths(markdown: &str, root: Option<&Path>) -> String {
    let mut out = String::with_capacity(markdown.len());
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.split_inclusive('\n') {
        if let Some((marker, length)) = fence {
            out.push_str(line);
            let trimmed = line.trim();
            if trimmed.chars().take_while(|c| *c == marker).count() >= length
                && trimmed.chars().all(|c| c == marker || c.is_whitespace())
            {
                fence = None;
            }
            continue;
        }
        let trimmed = line.trim_start();
        if let Some((marker, length)) = fence_marker(trimmed) {
            fence = Some((marker, length));
            out.push_str(line);
            continue;
        }
        out.push_str(&linkify_line(line, root));
    }
    out
}

fn fence_marker(line: &str) -> Option<(char, usize)> {
    let marker = line.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let length = line.chars().take_while(|c| *c == marker).count();
    (length >= 3).then_some((marker, length))
}

fn linkify_line(line: &str, root: Option<&Path>) -> String {
    if !line.contains('`') {
        return line.to_string();
    }
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        let Some(offset) = line[cursor..].find('`') else {
            out.push_str(&line[cursor..]);
            break;
        };
        let open = cursor + offset;
        let run = bytes[open..]
            .iter()
            .take_while(|byte| **byte == b'`')
            .count();
        let content_start = open + run;
        let Some((content_end, close_end)) = closing_run(line, content_start, run) else {
            out.push_str(&line[cursor..content_start]);
            cursor = content_start;
            continue;
        };
        let content = &line[content_start..content_end];
        if let Some(link) = file_link(content.trim(), root) {
            out.push_str(&line[cursor..open]);
            out.push_str(&link);
        } else {
            out.push_str(&line[cursor..close_end]);
        }
        cursor = close_end;
    }
    out
}

fn closing_run(line: &str, from: usize, length: usize) -> Option<(usize, usize)> {
    let bytes = line.as_bytes();
    let mut cursor = from;
    while cursor < bytes.len() {
        if bytes[cursor] != b'`' {
            cursor += 1;
            continue;
        }
        let start = cursor;
        while cursor < bytes.len() && bytes[cursor] == b'`' {
            cursor += 1;
        }
        if cursor - start == length {
            return Some((start, cursor));
        }
    }
    None
}

fn file_link(raw: &str, root: Option<&Path>) -> Option<String> {
    let path = Path::new(raw);
    if raw.is_empty()
        || raw == "/"
        || raw.chars().any(char::is_whitespace)
        || raw.contains('`')
        || raw.contains('[')
        || raw.contains(']')
        || raw.contains('*')
    {
        return None;
    }
    let (resolved, label) = if path.is_absolute() {
        (path.to_path_buf(), display_path(path, root))
    } else if let Some(relative) = raw.strip_prefix("~/") {
        (home_dir()?.join(relative), raw.to_string())
    } else {
        (root?.join(path), raw.trim_start_matches("./").to_string())
    };
    if resolved.file_name().is_none() || !resolved.exists() {
        return None;
    }
    let url = Url::from_file_path(&resolved).ok()?;
    Some(format!("[{label}](<{}>)", url.as_str()))
}

fn display_path(path: &Path, root: Option<&Path>) -> String {
    if let Some(root) = root
        && let Ok(relative) = path.strip_prefix(root)
        && !relative.as_os_str().is_empty()
    {
        return relative.display().to_string();
    }
    if let Some(home) = home_dir()
        && let Ok(relative) = path.strip_prefix(&home)
        && !relative.as_os_str().is_empty()
    {
        return PathBuf::from("~").join(relative).display().to_string();
    }
    path.display().to_string()
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("desktop crate lives below the workspace root")
            .to_path_buf()
    }

    #[test]
    fn an_existing_absolute_path_becomes_a_compact_link() {
        let root = project_root();
        let file = root.join("crates/desktop/assets/app-icon.png");
        let markdown = format!("Open `{}` now.", file.display());

        let linked = linkify_paths(&markdown, Some(&root));

        assert!(linked.contains("[crates/desktop/assets/app-icon.png](<file://"));
        assert!(linked.contains("app-icon.png"));
    }

    #[test]
    fn a_relative_path_resolves_against_the_project_root() {
        let root = std::env::temp_dir().join(format!(
            "tact-file-links-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("charts")).unwrap();
        std::fs::write(root.join("charts/markets_outlook.svg"), "<svg/>").unwrap();
        let markdown = "- `charts/markets_outlook.svg` — 矢量版";

        let linked = linkify_paths(markdown, Some(&root));

        assert!(linked.contains("[charts/markets_outlook.svg](<file://"));
        assert!(!linked.contains("`charts/markets_outlook.svg`"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn code_blocks_and_missing_paths_keep_their_text() {
        let root = project_root();
        let file = root.join("crates/desktop/assets/app-icon.png");
        let fenced = format!("```\n`{}`\n```\n", file.display());

        assert_eq!(linkify_paths(&fenced, Some(&root)), fenced);
        assert_eq!(
            linkify_paths("`/definitely/not/here.png`", Some(&root)),
            "`/definitely/not/here.png`"
        );
    }
}
