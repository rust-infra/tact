//! Splits a unified diff into the files it touches.
//!
//! A single-file patch is best read as a patch. A run that edits several
//! files needs the multi-file review surface instead, so the reader asks this
//! module whether the patch is multi-file and, if it is, what the files and
//! their line counts are.

/// One file in a unified diff.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
}

/// Whether the patch touches more than one file.
pub fn is_multi_file(patch: &str) -> bool {
    files(patch).len() > 1
}

/// Splits a unified diff into per-file line counts.
pub fn files(patch: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<usize> = None;
    let mut in_hunk = false;
    for line in patch.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let path = rest
                .split_whitespace()
                .last()
                .unwrap_or(rest)
                .trim_start_matches("b/");
            files.push(FileDiff {
                path: path.to_string(),
                added: 0,
                removed: 0,
            });
            current = Some(files.len() - 1);
            in_hunk = false;
            continue;
        }
        if let Some(rest) = line.strip_prefix("+++ ") {
            let path = rest.trim_start_matches("b/");
            if path == "/dev/null" {
                continue;
            }
            match current {
                Some(index) => files[index].path = path.to_string(),
                None => {
                    files.push(FileDiff {
                        path: path.to_string(),
                        added: 0,
                        removed: 0,
                    });
                    current = Some(files.len() - 1);
                }
            }
            continue;
        }
        if line.starts_with("@@") {
            in_hunk = true;
            continue;
        }
        let Some(index) = current else {
            continue;
        };
        if !in_hunk {
            continue;
        }
        if line.starts_with('+') {
            files[index].added += 1;
        } else if line.starts_with('-') {
            files[index].removed += 1;
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::{FileDiff, files, is_multi_file};

    const PATCH: &str = "diff --git a/src/a.rs b/src/a.rs
--- a/src/a.rs
+++ b/src/a.rs
@@ -1,2 +1,3 @@
-old
+new
+extra
diff --git a/src/b.rs b/src/b.rs
--- a/src/b.rs
+++ b/src/b.rs
@@ -1 +1 @@
-gone
+kept
";

    #[test]
    fn counts_additions_and_removals_per_file() {
        assert_eq!(
            files(PATCH),
            vec![
                FileDiff {
                    path: "src/a.rs".into(),
                    added: 2,
                    removed: 1,
                },
                FileDiff {
                    path: "src/b.rs".into(),
                    added: 1,
                    removed: 1,
                },
            ]
        );
        assert!(is_multi_file(PATCH));
    }

    #[test]
    fn a_single_file_patch_stays_a_patch() {
        let single = "--- a/one.rs\n+++ b/one.rs\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(files(single).len(), 1);
        assert!(!is_multi_file(single));
        assert!(!is_multi_file(""));
    }

    #[test]
    fn header_lines_are_not_counted_as_changes() {
        let single = "--- a/one.rs\n+++ b/one.rs\n@@ -1 +1 @@\n-a\n+b\n";
        assert_eq!(files(single)[0].added, 1);
        assert_eq!(files(single)[0].removed, 1);
    }
}
