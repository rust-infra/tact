//! Shared utilities ported for future use.
//!
//! Currently unused by production call sites; kept as a self-contained
//! module so truncation helpers can be adopted without rewriting Codex logic.

pub mod output_truncation;
pub mod truncate;

pub use output_truncation::{
    TruncationPolicy, formatted_truncate_text, truncate_text,
};
pub use truncate::{
    approx_bytes_for_tokens, approx_token_count, approx_tokens_from_byte_count,
    truncate_middle_chars, truncate_middle_with_token_budget,
};
