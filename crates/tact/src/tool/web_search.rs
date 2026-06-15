// WebSearch tool: search the web using Brave Search API or fallback to DuckDuckGo.

use crate::tool::ToolContext;
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use tool_refactor_macros::tool;
use tracing::debug;

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

#[tool(
    name = "web_search",
    description = "Search the web for information. Returns a list of relevant web pages with \
                    titles, URLs, and snippets. Use this when you need current information \
                    not available in your training data, or when searching for documentation, \
                    examples, or news."
)]
pub async fn web_search(_ctx: ToolContext, input: WebSearchInput) -> Result<String> {
    let num_results = input.num_results.min(10).max(1);
    debug!(query = %input.query, num_results, "Web search");

    // Try Brave Search API first, then fall back to DuckDuckGo
    if let Some(api_key) = std::env::var("BRAVE_SEARCH_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
    {
        search_brave(&input.query, num_results, &api_key).await
    } else {
        search_duckduckgo(&input.query, num_results).await
    }
}

/// Search using the Brave Search API.
async fn search_brave(query: &str, num_results: usize, api_key: &str) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
        urlencoding_simple(query),
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

    Ok(format_brave_results(&data, num_results))
}

fn format_brave_results(data: &Value, max: usize) -> String {
    let mut output = String::new();
    let web_results = data
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array());

    if let Some(items) = web_results {
        for (i, item) in items.iter().take(max).enumerate() {
            let title = item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("(No title)");
            let url = item.get("url").and_then(|u| u.as_str()).unwrap_or("");
            let snippet = item
                .get("description")
                .and_then(|s| s.as_str())
                .unwrap_or("");

            output.push_str(&format!(
                "{}. **{}**\n   URL: {}\n   {}\n\n",
                i + 1,
                title,
                url,
                snippet
            ));
        }
    }

    if output.is_empty() {
        "No results found.".to_string()
    } else {
        output
    }
}

/// Fallback: DuckDuckGo Instant Answer API.
async fn search_duckduckgo(query: &str, num_results: usize) -> Result<String> {
    let client = reqwest::Client::new();
    let url = format!(
        "https://api.duckduckgo.com/?q={}&format=json&no_html=1&skip_disambig=1",
        urlencoding_simple(query)
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

    Ok(format_ddg_results(&data, num_results))
}

fn format_ddg_results(data: &Value, max: usize) -> String {
    let mut output = String::new();
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
            output.push_str(&format!(
                "**{}**\n{}\nURL: {}\n\n",
                source, abstract_text, url
            ));
            count += 1;
        }
    }

    // Related topics
    if let Some(topics) = data.get("RelatedTopics").and_then(|t| t.as_array()) {
        for topic in topics.iter().take(max.saturating_sub(count)) {
            if let Some(text) = topic.get("Text").and_then(|t| t.as_str()) {
                if !text.is_empty() {
                    let url = topic.get("FirstURL").and_then(|u| u.as_str()).unwrap_or("");
                    output.push_str(&format!("- {}\n  {}\n\n", text, url));
                }
            }
        }
    }

    if output.is_empty() {
        format!(
            "No instant answer found for '{}'. Try using the Brave Search API \
             by setting the BRAVE_SEARCH_API_KEY environment variable for full web search.",
            data.get("QuerySearchQuery")
                .and_then(|q| q.as_str())
                .unwrap_or("your query")
        )
    } else {
        output
    }
}

/// Minimal percent-encoding for URL query parameters.
fn urlencoding_simple(s: &str) -> String {
    let mut encoded = String::new();
    for ch in s.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => {
                encoded.push(ch);
            }
            ' ' => encoded.push('+'),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
        }
    }
    encoded
}
