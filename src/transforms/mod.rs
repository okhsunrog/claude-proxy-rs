//! Request/response transformations for the Anthropic API proxy.
//!
//! This module provides:
//! - `prepare`: Prepare any request for Anthropic API (system injection, user ID, etc.)
//! - `openai_compat`: OpenAI ↔ Anthropic format conversion
//! - `streaming`: SSE stream transformations

pub mod billing;
pub mod cache_policy;
pub mod openai_compat;
pub mod prepare;
pub mod streaming;
pub mod tool_adapters;
pub mod tool_aliases;
pub mod tool_events;

pub use openai_compat::{transform_openai_request, transform_openai_response};
#[cfg(test)]
pub use prepare::prepare_anthropic_request;
pub use streaming::{
    stream_anthropic_to_openai_with_usage, stream_restore_native_tool_names_with_usage,
};
pub use tool_aliases::restore_response_tool_names;

pub mod responses;
