use std::path::Path;

use anthropic_ai_sdk::types::message::{ContentBlock, ImageSource, Message, Role::User};
use base64::Engine as _;
use regex::Regex;

/// Parse inline markdown image references (`![alt](path.png)`) and `@` file
/// references (`@path/to/file` or `@"path with spaces"`) in the user's task.
///
/// Images are base64-encoded and attached as vision blocks; text files are
/// embedded as file content blocks. References that cannot be resolved are left
/// in the text unchanged.
pub(crate) async fn build_user_message(task: &str, work_dir: &Path) -> Message {
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
                let resolved = work_dir.join(path);
                match load_image_block(&resolved).await {
                    Some(source) => {
                        if !alt.is_empty() {
                            blocks.push(ContentBlock::Text {
                                text: format!("({})", alt),
                            });
                        }
                        blocks.push(ContentBlock::Image { source });
                    }
                    None => {
                        blocks.push(ContentBlock::Text {
                            text: task[content_start..end].to_string(),
                        });
                    }
                }
            }
            Ref::AtFile { path } => {
                let resolved = work_dir.join(path);
                if let Some(source) = load_image_block(&resolved).await {
                    blocks.push(ContentBlock::Image { source });
                } else if let Some(content) = load_text_file(&resolved).await {
                    let header = format!("--- file: {} ---\n", resolved.display());
                    blocks.push(ContentBlock::Text {
                        text: format!("{}{}\n---\n", header, content),
                    });
                } else {
                    blocks.push(ContentBlock::Text {
                        text: task[content_start..end].to_string(),
                    });
                }
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

async fn load_text_file(path: &Path) -> Option<String> {
    if !path.is_file() {
        return None;
    }
    tokio::fs::read_to_string(path).await.ok()
}

async fn load_image_block(path: &Path) -> Option<ImageSource> {
    if !path.is_file() {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => return None,
    };
    Some(ImageSource {
        type_: "base64".to_string(),
        media_type: media_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anthropic_ai_sdk::types::message::MessageContent;
    use std::path::PathBuf;

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
            ContentBlock::Text { text } => assert!(
                text.contains(needle),
                "expected text block to contain {:?}, got {:?}",
                needle,
                text
            ),
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
    fn test_at_text_file_embeds_content() {
        let dir = temp_dir();
        let file_path = dir.join("hello.txt");
        std::fs::write(&file_path, "hello world").unwrap();

        let msg =
            rt().block_on(async { build_user_message("review @hello.txt please", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 3);
        assert_text_contains(blocks[0], "review ");
        assert_text_contains(blocks[1], "--- file:");
        assert_text_contains(blocks[1], "hello world");
        assert_text_contains(blocks[2], "please");
    }

    #[test]
    fn test_at_image_file_attaches_vision_block() {
        let dir = temp_dir();
        let file_path = dir.join("pixel.png");
        std::fs::write(&file_path, b"not-really-a-png").unwrap();

        let msg = rt().block_on(async { build_user_message("look at @pixel.png", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 2);
        assert_text_contains(blocks[0], "look at ");
        assert!(matches!(blocks[1], ContentBlock::Image { .. }));
    }

    #[test]
    fn test_at_quoted_path_with_spaces() {
        let dir = temp_dir();
        let file_path = dir.join("my file.txt");
        std::fs::write(&file_path, "spacy content").unwrap();

        let msg =
            rt().block_on(async { build_user_message("read @\"my file.txt\" now", &dir).await });

        let blocks = text_blocks(&msg);
        assert_eq!(blocks.len(), 3);
        assert_text_contains(blocks[1], "spacy content");
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
    fn test_combined_markdown_image_and_at_file() {
        let dir = temp_dir();
        std::fs::write(dir.join("code.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("shot.png"), b"fake").unwrap();

        let msg = rt().block_on(async {
            build_user_message("check ![shot](shot.png) and @code.rs", &dir).await
        });

        let blocks = text_blocks(&msg);
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Image { .. }))
        );
        assert!(
            blocks
                .iter()
                .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("fn main")))
        );
    }
}
