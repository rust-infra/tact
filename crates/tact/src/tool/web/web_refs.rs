//! Shared helpers for linking `web_search` results to `web_fetch` via result ids.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

use tracing::warn;

thread_local! {
    static TEST_WEB_CACHE_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn web_cache_dir() -> PathBuf {
    if let Some(dir) = TEST_WEB_CACHE_DIR.with(|cell| cell.borrow().clone()) {
        return dir;
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".tact").join("web_cache")
}

fn search_ref_dir() -> PathBuf {
    web_cache_dir().join("search_refs")
}

#[cfg(test)]
pub(crate) fn set_test_web_cache_dir(dir: Option<PathBuf>) {
    TEST_WEB_CACHE_DIR.with(|cell| *cell.borrow_mut() = dir);
}

#[cfg(test)]
pub(crate) fn with_test_web_cache<F: FnOnce()>(name: &str, f: F) {
    let cache_dir = std::env::temp_dir().join(format!("tact-web-cache-test-{name}"));
    let _ = fs::remove_dir_all(&cache_dir);
    fs::create_dir_all(&cache_dir).unwrap();
    set_test_web_cache_dir(Some(cache_dir.clone()));
    f();
    set_test_web_cache_dir(None);
    let _ = fs::remove_dir_all(&cache_dir);
}

pub(crate) fn search_result_id(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("ws_{:x}", hasher.finish())
}

pub(crate) fn is_valid_result_id(result_id: &str) -> bool {
    if !result_id.starts_with("ws_") {
        return false;
    }
    let suffix = &result_id[3..];
    !suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_hexdigit())
}

pub(crate) fn save_search_reference(result_id: &str, url: &str) {
    let dir = search_ref_dir();
    if let Err(err) = fs::create_dir_all(&dir) {
        warn!(dir = ?dir, error = %err, "Failed to create search reference directory");
        return;
    }

    let file = dir.join(format!("{result_id}.txt"));
    if let Err(err) = fs::write(&file, url) {
        warn!(file = ?file, error = %err, "Failed to write search reference file");
    }
}

pub(crate) fn load_search_reference(result_id: &str) -> Option<String> {
    let trimmed = result_id.trim();
    if !is_valid_result_id(trimmed) {
        return None;
    }
    let file = search_ref_dir().join(format!("{trimmed}.txt"));
    fs::read_to_string(file)
        .ok()
        .map(|content| content.trim().to_string())
}

pub(crate) fn resolve_fetch_target(
    url: Option<&str>,
    result_id: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(url) = url.map(str::trim).filter(|value| !value.is_empty()) {
        return Ok(url.to_string());
    }

    if let Some(result_id) = result_id.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(url) = load_search_reference(result_id) {
            return Ok(url);
        }
        anyhow::bail!(
            "Unknown result_id: {result_id}. Run web_search first or provide url directly."
        );
    }

    anyhow::bail!("web_fetch requires either `url` or `result_id`.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    #[derive(serde::Serialize)]
    struct SearchResult {
        #[serde(rename = "result_id")]
        id: String,
        title: String,
        url: String,
        snippet: String,
    }

    #[derive(serde::Serialize)]
    struct SearchResultPayload<'a> {
        query: &'a str,
        results: &'a [SearchResult],
    }

    fn format_and_persist(query: &str, results: &[SearchResult]) -> String {
        let mut output = String::new();
        let _ = writeln!(&mut output, "Search results for \"{query}\":");
        let _ = writeln!(&mut output);

        for (index, result) in results.iter().enumerate() {
            save_search_reference(&result.id, &result.url);
            let _ = writeln!(
                &mut output,
                "{}. [{}] **{}**",
                index + 1,
                result.id,
                result.title
            );
            let _ = writeln!(&mut output, "   URL: {}", result.url);
            if !result.snippet.is_empty() {
                let _ = writeln!(&mut output, "   {}", result.snippet);
            }
            let _ = writeln!(&mut output);
        }

        let payload = SearchResultPayload { query, results };
        let payload_json =
            serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());
        let _ = writeln!(&mut output, "JSON_RESULTS_BEGIN");
        let _ = writeln!(&mut output, "{payload_json}");
        let _ = writeln!(&mut output, "JSON_RESULTS_END");
        output.trim_end().to_string()
    }

    #[test]
    fn search_to_fetch_result_id_roundtrip() {
        with_test_web_cache("search_to_fetch_result_id_roundtrip", || {
            let url = "https://example.com/e2e-test-page";
            let result_id = search_result_id(url);
            let results = vec![SearchResult {
                id: result_id.clone(),
                title: "Example".to_string(),
                url: url.to_string(),
                snippet: "Snippet".to_string(),
            }];

            let output = format_and_persist("example query", &results);
            assert!(output.contains(&result_id));

            let resolved = resolve_fetch_target(None, Some(&result_id)).unwrap();
            assert_eq!(resolved, url);
        });
    }

    #[test]
    fn search_json_block_matches_persisted_result_id() {
        with_test_web_cache("search_json_block_matches_persisted_result_id", || {
            let url = "https://example.com/json-test";
            let result_id = search_result_id(url);
            let results = vec![SearchResult {
                id: result_id.clone(),
                title: "JSON Test".to_string(),
                url: url.to_string(),
                snippet: String::new(),
            }];

            let output = format_and_persist("json query", &results);
            let start = output.find("JSON_RESULTS_BEGIN").unwrap();
            let end = output.find("JSON_RESULTS_END").unwrap();
            let json = output[start + "JSON_RESULTS_BEGIN".len()..end].trim();
            let parsed: serde_json::Value = serde_json::from_str(json).unwrap();

            assert_eq!(parsed["results"][0]["result_id"], result_id);
            assert_eq!(
                resolve_fetch_target(
                    None,
                    Some(parsed["results"][0]["result_id"].as_str().unwrap())
                )
                .unwrap(),
                url
            );
        });
    }
}
