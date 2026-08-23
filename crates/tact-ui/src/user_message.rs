use std::path::Path;

use regex::Regex;
use tact_llm::{ContentBlock, Message, Role::User};

/// Parse inline markdown image references (`![alt](path.png)`) and `@` file
/// references (`@path/to/file` or `@"path with spaces"`) in the user's task.
///
/// **De-inlined:** image and file references are kept as path text so the
/// model reads them on demand via `read_image` (image) / `read_file` (text)
/// rather than base64-inlining or whole-file inlining at attach time.
pub(crate) async fn build_user_message(task: &str, _work_dir: &Path) -> Message {
    static REF_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = REF_RE.get_or_init(|| {
        Regex::new(r#"(?m)(?P<prefix>^|[ \t])(?:(?P<img>!\[(?P<alt>[^\]]*)\]\((?P<img_path>[^)]+)\))|@(?:"(?P<qpath>[^"]+)"|(?P<upath>\S+)))"#).unwrap()
    });

    #[derive(Debug)]
    enum Ref<'a> {
        Image { alt: &'a str, path: &'a str },
        AtFile { path: &'a str },
    }

    let mut refs = Vec::new();
    for cap in re.captures_iter(task) {
        let m = cap.get(0).unwrap();
        let prefix_len = cap.name("prefix").map(|m| m.as_str().len()).unwrap_or(0);
        if cap.name("img").is_some() {
            let alt = cap.name("alt").map(|m| m.as_str()).unwrap_or("");
            let path = cap.name("img_path").map(|m| m.as_str()).unwrap_or("");
            refs.push((m.start(), m.end(), prefix_len, Ref::Image { alt, path }));
        } else {
            let path = cap
                .name("qpath")
                .or_else(|| cap.name("upath"))
                .map(|m| m.as_str())
                .unwrap_or("");
            refs.push((m.start(), m.end(), prefix_len, Ref::AtFile { path }));
        }
    }
    refs.sort_by_key(|(s, _, _, _)| *s);

    let mut blocks = Vec::new();
    let mut last_end = 0usize;

    for (start, end, prefix_len, r) in refs {
        let content_start = start + prefix_len;
        if content_start > last_end {
            blocks.push(ContentBlock::Text {
                text: task[last_end..content_start].to_string(),
            });
        }

        match r {
            Ref::Image { alt, path } => {
                // Harness-style "de-inline": keep the image as a path reference
                // instead of base64-inlining it. The model calls `read_image`
                // on demand when it needs the pixels.
                blocks.push(ContentBlock::Text {
                    text: format!("![{}]({})", alt, path),
                });
            }
            Ref::AtFile { path } => {
                // De-inline: keep the file as a path reference; the model uses
                // `read_file` (text) or `read_image` (image) on demand.
                blocks.push(ContentBlock::Text {
                    text: format!("@{}", path),
                });
            }
        }

        last_end = end;
    }

    if last_end < task.len() {
        blocks.push(ContentBlock::Text {
            text: task[last_end..].to_string(),
        });
    }

    if blocks.is_empty() {
        return Message::new_text(User, "");
    }

    Message::new_blocks(User, blocks)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tact_llm::MessageContent;

    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tact_tui_test_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn assert_text_contains(block: &ContentBlock, needle: &str) {
        match block {
            ContentBlock::Text { text } => {
                assert!(
                    text.contains(needle),
                    "expected text block to contain {:?}, got {:?}",
                    needle,
                    text
                )
            }
            _ => panic!("expected text block, got {:?}", block),
        }
    }

    fn text_blocks(msg: &Message) -> Vec<&ContentBlock> {
        match &msg.content {
            MessageContent::Blocks { content } => content.iter().collect(),
            _ => panic!("expected block content"),
        }
    }

    #[test]
    fn test_at_path_outside_workspace_left_unchanged() {
        let dir = temp_dir();
        std::fs::write(dir.join("local.txt"), "local").unwrap();

        let msg = rt().block_on(async { build_user_message("read @../outside.txt", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 2);
        assert_text_contains(blocks[0], "read ");
        assert_text_contains(blocks[1], "@../outside.txt");
    }

    #[test]
    fn test_at_text_file_deinlined_to_path() {
        let dir = temp_dir();
        std::fs::write(dir.join("hello.txt"), "hello world").unwrap();

        let msg =
            rt().block_on(async { build_user_message("review @hello.txt please", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 3);
        assert_text_contains(blocks[0], "review ");
        assert_text_contains(blocks[1], "@hello.txt");
        assert_text_contains(blocks[2], "please");
    }

    #[test]
    fn test_at_image_file_deinlined_to_path() {
        let dir = temp_dir();
        std::fs::write(dir.join("pixel.png"), b"not-read").unwrap();

        let msg = rt().block_on(async { build_user_message("look at @pixel.png", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 2);
        assert_text_contains(blocks[0], "look at ");
        assert_text_contains(blocks[1], "@pixel.png");
    }

    #[test]
    fn test_at_quoted_path_with_spaces() {
        let dir = temp_dir();
        std::fs::write(dir.join("my file.txt"), "spacy content").unwrap();

        let msg =
            rt().block_on(async { build_user_message("read @\"my file.txt\" now", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 3);
        assert_text_contains(blocks[1], "@my file.txt");
        assert_text_contains(blocks[2], "now");
    }

    #[test]
    fn test_at_missing_file_left_unchanged() {
        let dir = temp_dir();

        let msg = rt().block_on(async { build_user_message("see @missing.txt", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 2);
        assert_text_contains(blocks[0], "see ");
        assert_text_contains(blocks[1], "@missing.txt");
    }

    #[test]
    fn test_combined_markdown_image_and_at_file_deinlined() {
        let dir = temp_dir();
        std::fs::write(dir.join("code.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("shot.png"), b"not-read").unwrap();

        let msg = rt().block_on(async {
            build_user_message("check ![shot](shot.png) and @code.rs", &dir).await
        });

        let blocks = text_blocks(&msg);
        assert!(blocks.iter().any(
            |b| matches!(b, ContentBlock::Text { text } if text.contains("![shot](shot.png)"))
        ));
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("@code.rs")))
        );
        // De-inlined: no image block, no inlined file content.
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        );
        assert!(
            !blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("fn main")))
        );
    }
}
