//! Request/response transformations for the Anthropic API proxy.
//!
//! This module provides:
//! - `common`: Shared utilities (fake user ID generation, cache control counting)
//! - `tool_names`: mcp_ prefix handling for OAuth
//! - `prepare`: Prepare any request for Anthropic API (system injection, user ID, etc.)
//! - `openai_compat`: OpenAI ↔ Anthropic format conversion
//! - `streaming`: SSE stream transformations

pub mod common;
pub mod openai_compat;
pub mod prepare;
pub mod streaming;
pub mod tool_names;

// Re-export commonly used items
pub use openai_compat::{
    AnthropicResponse, AnthropicUsage, OpenAIChatRequest, transform_openai_request,
    transform_openai_response,
};
pub use prepare::{prepare_anthropic_request, prepare_count_tokens_request};
pub use streaming::{stream_anthropic_to_openai_with_usage, stream_strip_mcp_prefix_with_usage};
