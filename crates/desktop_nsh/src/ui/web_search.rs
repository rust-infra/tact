//! Turns a web-search tool's output into `SearchResults` rows.
//!
//! MCP search servers return JSON, Markdown link lists, or a plain list of
//! URLs depending on the provider. The reader shows all three through the
//! same component: every hit keeps its URL (the row ID), a title and the
//! source domain.

use std::collections::HashSet;

/// One search hit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub url: String,
    pub title: String,
    pub domain: String,
}

/// Whether a tool name is a web search.
pub fn is_search_tool(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("search") || name == "web"
}

/// The query shown above the results, read from the tool's arguments.
pub fn query(tool_name: &str, arguments: &str) -> String {
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(arguments) {
        for key in ["query", "q", "search", "term"] {
            if let Some(query) = value.get(key).and_then(|value| value.as_str()) {
                return query.to_string();
            }
        }
    }
    let trimmed = arguments.trim().trim_matches('"');
    if trimmed.is_empty() {
        tool_name.to_string()
    } else {
        trimmed.to_string()
    }
}

/// Parses the tool output into unique hits.
pub fn parse(output: &str) -> Vec<Hit> {
    let mut hits = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        collect_json(&value, &mut hits);
    }
    if hits.is_empty() {
        collect_markdown_links(output, &mut hits);
    }
    if hits.is_empty() {
        collect_bare_urls(output, &mut hits);
    }
    let mut seen = HashSet::new();
    hits.retain(|hit| seen.insert(hit.url.clone()));
    hits
}

fn collect_json(value: &serde_json::Value, hits: &mut Vec<Hit>) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                collect_json(item, hits);
            }
        }
        serde_json::Value::Object(map) => {
            let url = ["url", "link", "href"]
                .iter()
                .find_map(|key| map.get(*key).and_then(|value| value.as_str()));
            if let Some(url) = url.filter(|url| url.starts_with("http")) {
                let title = ["title", "name", "heading"]
                    .iter()
                    .find_map(|key| map.get(*key).and_then(|value| value.as_str()))
                    .unwrap_or(url);
                hits.push(Hit {
                    url: url.to_string(),
                    title: title.to_string(),
                    domain: domain(url),
                });
            }
            for value in map.values() {
                collect_json(value, hits);
            }
        }
        _ => {}
    }
}

fn collect_markdown_links(output: &str, hits: &mut Vec<Hit>) {
    let mut rest = output;
    while let Some(open) = rest.find('[') {
        let Some(close) = rest[open..].find(']') else {
            break;
        };
        let title = &rest[open + 1..open + close];
        let after = &rest[open + close + 1..];
        if let Some(link) = after.strip_prefix('(')
            && let Some(end) = link.find(')')
        {
            let url = link[..end].trim();
            if url.starts_with("http") {
                hits.push(Hit {
                    url: url.to_string(),
                    title: if title.trim().is_empty() {
                        url.to_string()
                    } else {
                        title.trim().to_string()
                    },
                    domain: domain(url),
                });
            }
        }
        rest = &rest[open + close + 1..];
    }
}

fn collect_bare_urls(output: &str, hits: &mut Vec<Hit>) {
    let mut previous_line = "";
    for line in output.lines() {
        let line = line.trim();
        let url = line
            .split_whitespace()
            .find(|token| token.starts_with("http://") || token.starts_with("https://"));
        if let Some(url) = url {
            let url = url.trim_end_matches(['.', ',', ')', ']', ';']);
            let title = if previous_line.is_empty() || previous_line.starts_with("http") {
                url
            } else {
                previous_line
            };
            hits.push(Hit {
                url: url.to_string(),
                title: title.to_string(),
                domain: domain(url),
            });
        }
        previous_line = line;
    }
}

fn domain(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or(rest)
        .trim_start_matches("www.")
        .to_string()
}


