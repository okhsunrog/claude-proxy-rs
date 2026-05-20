//! Request/response transformations for the Anthropic API proxy.
//!
//! This module provides:
//! - `prepare`: Prepare any request for Anthropic API (system injection, user ID, etc.)
//! - `openai_compat`: OpenAI ↔ Anthropic format conversion
//! - `streaming`: SSE stream transformations

pub mod openai_compat;
pub mod prepare;
pub mod streaming;
pub mod tool_aliases;

pub use openai_compat::{transform_openai_request, transform_openai_response};
pub use prepare::{prepare_anthropic_request, prepare_count_tokens_request};
pub use streaming::{
    stream_anthropic_to_openai_with_usage, stream_restore_native_tool_names_with_usage,
};
pub use tool_aliases::{
    ToolNameMap, normalize_claude_code_tool_names, restore_response_tool_names,
};
