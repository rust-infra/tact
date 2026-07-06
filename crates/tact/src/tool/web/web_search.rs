// WebSearch tool: search the web using Brave Search API or fallback to DuckDuckGo.
//
// Returns structured results with stable `result_id` values. `web_fetch` can
// consume those ids directly, allowing a search -> fetch flow without forcing
// the model to copy/paste raw URLs.

use crate::tool::ToolContext;
use super::{http, web_refs};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use std::fmt::Write as _;
use tool_refactor_macros::tool;
use tracing::{debug, warn};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WebSearchInput {
    #[schemars(description = "The search query.")]
    pub query: String,
    #[schemars(description = "Number of results to return (default: 5, max: 10).")]
    #[serde(default = "default_num_results")]
    pub num_results: usize,
}

fn default_num_results() -> usize {
    5
}

#[derive(Debug, Clone, Serialize)]
struct SearchResult {
    #[serde(rename = "result_id")]
    id: String,
    title: String,
    url: String,
    snippet: String,
}

#[derive(Debug, Serialize)]
struct SearchResultPayload<'a> {
    query: &'a str,
    results: &'a [SearchResult],
}

const DDG_LIMITED_NOTICE: &str = "Note: DuckDuckGo instant answers only (not full web search). \
                                   Set BRAVE_SEARCH_API_KEY for full results.\n\n";

fn format_results(query: &str, results: &[SearchResult], persist_refs: bool) -> String {
    if results.is_empty() {
        return format!(
            "No instant answer found for '{query}'. Configure BRAVE_SEARCH_API_KEY for full web search."
        );
    }

    let mut output = String::new();
    let _ = writeln!(&mut output, "Search results for \"{query}\":");
    let _ = writeln!(&mut output);

    for (index, result) in results.iter().enumerate() {
        if persist_refs {
            web_refs::save_search_reference(&result.id, &result.url);
        }

        let _ = writeln!(&mut output, "{}. [{}] **{}**", index + 1, result.id, result.title);
        let _ = writeln!(&mut output, "   URL: {}", result.url);
        if !result.snippet.is_empty() {
            let _ = writeln!(&mut output, "   {}", result.snippet);
        }
        let _ = writeln!(&mut output);
    }

    let payload = SearchResultPayload { query, results };
    let payload_json = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());

    let _ = writeln!(&mut output, "JSON_RESULTS_BEGIN");
    let _ = writeln!(&mut output, "{payload_json}");
    let _ = writeln!(&mut output, "JSON_RESULTS_END");

    output.trim_end().to_string()
}

#[tool(
    name = "web_search",
    description = "Search the web for information. With BRAVE_SEARCH_API_KEY configured, \
                    returns full web results (title, URL, snippet). Without it, falls back to \
                    DuckDuckGo instant answers only, which may omit most web pages."
)]
pub async fn web_search(_ctx: ToolContext, input: WebSearchInput) -> Result<String> {
    let num_results = input.num_results.min(10).max(1);
    debug!(query = %input.query, num_results, "Web search");

    if let Some(api_key) = crate::config::settings()
        .tools
        .brave_search_api_key
        .as_deref()
        .filter(|k| !k.is_empty())
    {
        match search_brave(&input.query, num_results, api_key).await {
            Ok(results) => return Ok(format_results(&input.query, &results, true)),
            Err(error) => {
                warn!(error = %error, "Brave search failed; falling back to DuckDuckGo");
                let fallback_results = search_duckduckgo(&input.query, num_results).await?;
                let fallback = format_results(&input.query, &fallback_results, true);
                return Ok(format!(
                    "Note: Brave Search failed ({error}); showing DuckDuckGo instant answers instead.\n\n{fallback}"
                ));
            }
        }
    }

    let fallback = format_results(&input.query, &search_duckduckgo(&input.query, num_results).await?, true);
    Ok(format!("{DDG_LIMITED_NOTICE}{fallback}"))
}

/// Search using the Brave Search API.
async fn search_brave(query: &str, num_results: usize, api_key: &str) -> Result<Vec<SearchResult>> {
    let client = http::http_client();
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        http::encode_query(query),
        num_results
    );

    let resp = client
        .get(&url)
        .header("Accept", "application/json")
        .header("Accept-Encoding", "gzip")
        .header("X-Subscription-Token", api_key)
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Search request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(anyhow::anyhow!(
            "Brave Search API returned status {}",
            status
        ));
    }

    let data: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    Ok(extract_brave_results(&data, num_results))
}

fn extract_brave_results(data: &Value, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let web_results = data
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array());

    if let Some(items) = web_results {
        for item in items.iter().take(max) {
            let title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("(No title)");
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.is_empty() {
                continue;
            }
            let snippet = item
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            results.push(SearchResult {
                id: web_refs::search_result_id(url),
                title: title.to_string(),
                url: url.to_string(),
                snippet: snippet.to_string(),
            });
        }
    }
    results
}

/// Fallback: DuckDuckGo Instant Answer API.
async fn search_duckduckgo(query: &str, num_results: usize) -> Result<Vec<SearchResult>> {
    let client = http::http_client();
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        http::encode_query(query)
    );

    let resp = client
        .get(&url)
        .header("User-Agent", "tact/1.0")
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Search request failed: {}", e))?;

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        return Err(anyhow::anyhow!("DuckDuckGo API returned status {}", status));
    }

    let data: Value = resp
        .json()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

    Ok(extract_ddg_results(&data, num_results))
}

fn extract_ddg_results(data: &Value, max: usize) -> Vec<SearchResult> {
    let mut results = Vec::new();
    let mut count = 0;

    // Abstract (main answer)
    if let Some(abstract_text) = data.get("Abstract").and_then(|a| a.as_str()) {
        if !abstract_text.is_empty() {
            let source = data
                .get("AbstractSource")
                .and_then(|s| s.as_str())
                .unwrap_or("");
            let url = data
                .get("AbstractURL")
                .and_then(|u| u.as_str())
                .unwrap_or("");
            if !url.is_empty() {
                results.push(SearchResult {
                    id: web_refs::search_result_id(url),
                    title: if source.is_empty() {
                        "DuckDuckGo abstract".to_string()
                    } else {
                        source.to_string()
                    },
                    url: url.to_string(),
                    snippet: abstract_text.to_string(),
                });
                count += 1;
            }
        }
    }

    // Related topics
    if let Some(topics) = data.get("RelatedTopics").and_then(|t| t.as_array()) {
        for topic in topics.iter().take(max.saturating_sub(count)) {
            if let Some(text) = topic.get("Text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    let url = topic.get("FirstURL").and_then(|u| u.as_str()).unwrap_or("");
                    if url.is_empty() {
                        continue;
                    }
                    results.push(SearchResult {
                        id: web_refs::search_result_id(url),
                        title: text.to_string(),
                        url: url.to_string(),
                        snippet: String::new(),
                    });
                }
            }
        }
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_brave_results_renders_items() {
        let data = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "Example",
                        "url": "https://example.com",
                        "description": "Snippet"
                    }
                ]
            }
        });

        let results = extract_brave_results(&data, 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example");
        assert_eq!(results[0].url, "https://example.com");
        assert_eq!(results[0].snippet, "Snippet");
        assert!(results[0].id.starts_with("ws_"));
    }

    #[test]
    fn format_results_includes_stable_result_id() {
        let results = vec![SearchResult {
            id: web_refs::search_result_id("https://example.com/docs"),
            title: "Example".to_string(),
            url: "https://example.com/docs".to_string(),
            snippet: "Snippet".to_string(),
        }];
        let output = format_results("example query", &results, false);
        assert!(output.contains("Search results for \"example query\":"));
        assert!(output.contains("[ws_"));
        assert!(output.contains("URL: https://example.com/docs"));
        assert!(output.contains("JSON_RESULTS_BEGIN"));
        assert!(output.contains("\"result_id\""));
        assert!(output.contains("JSON_RESULTS_END"));
    }

    #[test]
    fn format_results_json_block_is_parseable() {
        let results = vec![SearchResult {
            id: web_refs::search_result_id("https://example.com/docs"),
            title: "Example".to_string(),
            url: "https://example.com/docs".to_string(),
            snippet: "Snippet".to_string(),
        }];

        let output = format_results("example query", &results, false);
        let start = output.find("JSON_RESULTS_BEGIN").unwrap();
        let end = output.find("JSON_RESULTS_END").unwrap();
        let json = output[start + "JSON_RESULTS_BEGIN".len()..end].trim();
        let parsed: Value = serde_json::from_str(json).unwrap();
        assert_eq!(parsed["query"], "example query");
        assert_eq!(parsed["results"][0]["result_id"], results[0].id);
    }

    #[test]
    fn format_results_explains_missing_full_search() {
        let data = serde_json::json!({ "Heading": "rust lang" });
        let output = format_results("rust lang", &extract_ddg_results(&data, 5), false);
        assert!(output.contains("No instant answer found"));
        assert!(output.contains("Configure BRAVE_SEARCH_API_KEY"));
    }

    #[test]
    fn search_result_id_is_stable() {
        let one = web_refs::search_result_id("https://example.com/docs");
        let two = web_refs::search_result_id("https://example.com/docs");
        assert_eq!(one, two);
    }
}
