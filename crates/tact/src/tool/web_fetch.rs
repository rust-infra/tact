// WebFetch tool: HTTP GET with HTML-to-text conversion and caching.
//
// Strips HTML tags, decodes common entities, collapses whitespace, and caches
// results under ~/.tact/web_cache/.  Edge-case detection for JS-heavy pages
// is present but semantic (LLM-based) extraction is not yet wired up — it
// depends on `claurst_api::AnthropicClient` which is not available in tact.

use crate::tool::{http::public_fetch_client, http::validate_public_http_url_resolved, web_refs, ToolContext};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::{fs, time::Duration};
use tool_refactor_macros::tool;
use tracing::{debug, warn};

const MAX_CONTENT_CHARS: usize = 100_000;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebFetchInput {
    #[schemars(description = "Public http(s) URL to fetch.")]
    pub url: Option<String>,
    #[schemars(description = "Optional result id from web_search output, e.g. ws_ab12cd.")]
    #[serde(default)]
    pub result_id: Option<String>,
    #[schemars(description = "Optional prompt for how to process the content.")]
    #[serde(default)]
    #[allow(dead_code)]
    pub prompt: Option<String>,
}

/// Compute a simple hash of the URL for cache purposes.
fn url_hash(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Get the cache directory for web_fetch content.
fn get_cache_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".tact").join("web_cache")
}

fn resolve_requested_url(input: &WebFetchInput) -> Result<String> {
    web_refs::resolve_fetch_target(
        input.url.as_deref(),
        input.result_id.as_deref(),
    )
}

/// Attempt to load cached extracted content for a URL.
fn load_cached_extraction(url: &str) -> Option<String> {
    let cache_dir = get_cache_dir();
    let cache_file = cache_dir.join(format!("{}.txt", url_hash(url)));

    if cache_file.exists() {
        match fs::read_to_string(&cache_file) {
            Ok(content) => {
                debug!(file = ?cache_file, "Loaded cached web content");
                return Some(content);
            }
            Err(e) => {
                debug!(file = ?cache_file, error = %e, "Failed to load cache");
            }
        }
    }
    None
}

/// Save extracted content to cache.
fn save_cached_extraction(url: &str, content: &str) {
    let cache_dir = get_cache_dir();
    if let Err(e) = fs::create_dir_all(&cache_dir) {
        warn!(dir = ?cache_dir, error = %e, "Failed to create cache directory");
        return;
    }

    let cache_file = cache_dir.join(format!("{}.txt", url_hash(url)));
    if let Err(e) = fs::write(&cache_file, content) {
        warn!(file = ?cache_file, error = %e, "Failed to write cache file");
    } else {
        debug!(file = ?cache_file, "Cached extracted web content");
    }
}

/// Detect if HTML is likely a JS-heavy page with minimal semantic content.
fn is_edge_case_html(html: &str, extracted_text: &str) -> bool {
    let word_count = extracted_text.split_whitespace().count();
    if word_count < 100 {
        debug!(word_count, "Edge case: low word count");
        return true;
    }

    let lower = html.to_lowercase();
    let has_semantic =
        lower.contains("<article") || lower.contains("<main") || lower.contains("<body");

    if !has_semantic {
        debug!("Edge case: no semantic HTML tags");
        return true;
    }

    false
}

/// Naively strip HTML tags and decode common entities.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && chars[i] == '<' {
            in_tag = true;
            let rest: String = lower_chars[i..].iter().take(20).collect();
            if rest.starts_with("<script") {
                in_script = true;
            } else if rest.starts_with("</script") {
                in_script = false;
            } else if rest.starts_with("<style") {
                in_style = true;
            } else if rest.starts_with("</style") {
                in_style = false;
            }
            // Block tags => newline
            let block_tags = [
                "<br", "<p ", "<p>", "</p>", "<div", "</div>", "<h1", "<h2", "<h3", "<h4", "<h5",
                "<h6", "</h1", "</h2", "</h3", "</h4", "</h5", "</h6", "<li", "</li", "<tr",
                "</tr", "<hr",
            ];
            for tag in &block_tags {
                if rest.starts_with(tag) {
                    result.push('\n');
                    break;
                }
            }
            i += 1;
            continue;
        }

        if in_tag {
            if chars[i] == '>' {
                in_tag = false;
            }
            i += 1;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        // Decode basic entities
        if chars[i] == '&' {
            let rest: String = chars[i..].iter().take(10).collect();
            if rest.starts_with("&amp;") {
                result.push('&');
                i += 5;
            } else if rest.starts_with("&lt;") {
                result.push('<');
                i += 4;
            } else if rest.starts_with("&gt;") {
                result.push('>');
                i += 4;
            } else if rest.starts_with("&quot;") {
                result.push('"');
                i += 6;
            } else if rest.starts_with("&#39;") || rest.starts_with("&apos;") {
                result.push('\'');
                i += if rest.starts_with("&#39;") { 5 } else { 6 };
            } else if rest.starts_with("&nbsp;") {
                result.push(' ');
                i += 6;
            } else {
                result.push('&');
                i += 1;
            }
            continue;
        }

        result.push(chars[i]);
        i += 1;
    }

    // Collapse multiple blank lines
    let mut collapsed = String::new();
    let mut blank_count = 0;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                collapsed.push('\n');
            }
        } else {
            blank_count = 0;
            collapsed.push_str(trimmed);
            collapsed.push('\n');
        }
    }

    collapsed.trim().to_string()
}

fn truncate_content(text: &str, max_len: usize) -> String {
    let total_chars = text.chars().count();
    if total_chars <= max_len {
        return text.to_string();
    }

    let truncated: String = text.chars().take(max_len).collect();
    format!(
        "{truncated}\n\n... (truncated, {total_chars} total characters)"
    )
}

#[tool(
    name = "web_fetch",
    description = "Fetches a public http(s) page and returns plain text. Accepts either a URL \
                    or a web_search result_id. HTML is converted \
                    to text; responses are cached locally. Only public URLs are allowed \
                    (localhost and private-network hosts are blocked). JS-heavy pages may \
                    return incomplete text."
)]
pub async fn web_fetch(_ctx: ToolContext, input: WebFetchInput) -> Result<String> {
    let request_url = resolve_requested_url(&input)?;
    debug!(url = %request_url, "Fetching web page");
    let url = validate_public_http_url_resolved(&request_url).await?;

    if let Some(cached) = load_cached_extraction(&request_url) {
        return Ok(cached);
    }

    let client = public_fetch_client();

    let resp = client
        .get(url.clone())
        .header("User-Agent", "tact/1.0")
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch {}: {}", request_url, e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "HTTP {} when fetching {}",
            status,
            request_url
        ));
    }

    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let body = resp
        .text()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to read response body: {}", e))?;

    let text = if content_type.contains("html") {
        let extracted = strip_html(&body);
        if is_edge_case_html(&body, &extracted) {
            debug!(
                url = %request_url,
                "JS-heavy page detected; basic HTML stripping may produce incomplete output"
            );
            format!(
                "[warning: page looks JS-heavy; extracted text may be incomplete]\n\n{extracted}"
            )
        } else {
            extracted
        }
    } else {
        body
    };

    let text = truncate_content(&text, MAX_CONTENT_CHARS);
    save_cached_extraction(&request_url, &text);

    Ok(text)
}

#[cfg(test)]
mod tests {
    use crate::tool::test_support::test_context;

    use super::*;

    #[test]
    fn resolve_requested_url_uses_explicit_url() {
        let input = WebFetchInput {
            url: Some("https://example.com".to_string()),
            result_id: None,
            prompt: None,
        };
        assert_eq!(
            resolve_requested_url(&input).unwrap(),
            "https://example.com".to_string()
        );
    }

    #[test]
    fn resolve_requested_url_requires_url_or_result_id() {
        let input = WebFetchInput {
            url: None,
            result_id: None,
            prompt: None,
        };
        let err = resolve_requested_url(&input).unwrap_err().to_string();
        assert!(err.contains("requires either `url` or `result_id`"));
    }

    #[test]
    fn resolve_requested_url_reports_unknown_result_id() {
        let input = WebFetchInput {
            url: None,
            result_id: Some("ws_missing".to_string()),
            prompt: None,
        };
        let err = resolve_requested_url(&input).unwrap_err().to_string();
        assert!(err.contains("Unknown result_id"));
    }

    #[test]
    fn resolve_requested_url_loads_persisted_web_search_reference() {
        web_refs::with_test_web_cache("web_fetch_ref", || {
            let url = "https://example.com/web-search-link";
            let result_id = web_refs::search_result_id(url);
            web_refs::save_search_reference(&result_id, url);

            let input = WebFetchInput {
                url: None,
                result_id: Some(result_id),
                prompt: None,
            };
            assert_eq!(resolve_requested_url(&input).unwrap(), url);
        });
    }

    #[test]
    fn result_id_validation_blocks_path_like_values() {
        assert!(web_refs::is_valid_result_id("ws_deadbeef"));
        assert!(!web_refs::is_valid_result_id("deadbeef"));
        assert!(!web_refs::is_valid_result_id("ws_../secret"));
        assert!(!web_refs::is_valid_result_id("ws_"));
        assert!(!web_refs::is_valid_result_id("ws_dead_beef"));
    }

    #[tokio::test]
    async fn web_fetch_does_not_bypass_host_validation_with_cache() {
        let context = test_context("web_fetch_does_not_bypass_host_validation_with_cache");
        let blocked_url = "http://127.0.0.1/private";
        save_cached_extraction(blocked_url, "should_not_be_returned");

        let err = web_fetch(
            context,
            WebFetchInput {
                url: Some(blocked_url.to_string()),
                result_id: None,
                prompt: None,
            },
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("Blocked host"));
    }

    #[test]
    fn truncate_content_reports_original_length() {
        let text = "x".repeat(MAX_CONTENT_CHARS + 50);
        let truncated = truncate_content(&text, MAX_CONTENT_CHARS);

        assert!(truncated.contains("... (truncated, 100050 total characters)"));
        assert!(truncated.chars().count() > MAX_CONTENT_CHARS);
    }

    #[test]
    fn truncate_content_leaves_short_text_unchanged() {
        let text = "hello";
        assert_eq!(truncate_content(text, MAX_CONTENT_CHARS), "hello");
    }
}
