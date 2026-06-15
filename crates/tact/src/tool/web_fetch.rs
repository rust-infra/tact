// WebFetch tool: HTTP GET with HTML-to-text conversion and caching.
//
// Strips HTML tags, decodes common entities, collapses whitespace, and caches
// results under ~/.tact/web_cache/.  Edge-case detection for JS-heavy pages
// is present but semantic (LLM-based) extraction is not yet wired up — it
// depends on `claurst_api::AnthropicClient` which is not available in tact.

use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::{fs, time::Duration};
use tool_refactor_macros::tool;
use tracing::{debug, warn};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebFetchInput {
    #[schemars(description = "The URL to fetch.")]
    pub url: String,
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

#[tool(
    name = "web_fetch",
    description = "Fetches a web page URL and returns its content as text. HTML is \
                    automatically converted to plain text. Use this for reading \
                    documentation, APIs, and other web resources."
)]
pub async fn web_fetch(_ctx: ToolContext, input: WebFetchInput) -> Result<String> {
    debug!(url = %input.url, "Fetching web page");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| anyhow::anyhow!("Failed to create HTTP client: {}", e))?;

    let resp = client
        .get(&input.url)
        .header("User-Agent", "tact/1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to fetch {}: {}", input.url, e))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!(
            "HTTP {} when fetching {}",
            status,
            input.url
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

    // Try to load from cache first
    if let Some(cached) = load_cached_extraction(&input.url) {
        return Ok(cached);
    }

    // Convert HTML to text if applicable
    let text = if content_type.contains("html") {
        strip_html(&body)
    } else {
        body.clone()
    };

    // Detect JS-heavy pages (SPAs, React/Vue apps).
    // Basic HTML stripping is used since tools don't have access to the LLM client;
    // the agent can follow up with a manual read if the output looks incomplete.
    if content_type.contains("html") && is_edge_case_html(&body, &text) {
        debug!(
            url = %input.url,
            "JS-heavy page detected; basic HTML stripping may produce incomplete output"
        );
    }

    // Truncate very long content
    const MAX_LEN: usize = 100_000;
    let text_chars = text.chars().take(MAX_LEN).collect::<String>();
    let text = if text_chars.len() > MAX_LEN {
        format!(
            "{}\n\n... (truncated, {} total characters)",
            text_chars,
            text_chars.len()
        )
    } else {
        text_chars
    };

    // Cache the final result
    save_cached_extraction(&input.url, &text);

    Ok(text)
}
