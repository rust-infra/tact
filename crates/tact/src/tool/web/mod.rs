//! Web tools: shared HTTP utilities, search, and fetch.

pub mod http;
mod web_fetch;
mod web_refs;
mod web_search;

pub use web_fetch::WebFetchTool;
pub use web_search::WebSearchTool;
