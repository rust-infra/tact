//! Shared utilities for approximate token budgets and middle truncation.
//!
//! `approx_token_count` is used by `read_file` pagination; the middle-truncation
//! helpers remain available for other tool-output paths.

pub mod output_truncation;
pub mod truncate;

pub use output_truncation::{
    TruncationPolicy, formatted_truncate_text, truncate_text,
};
pub use truncate::{
    approx_bytes_for_tokens, approx_token_count, approx_tokens_from_byte_count,
    truncate_middle_chars, truncate_middle_with_token_budget,
};
