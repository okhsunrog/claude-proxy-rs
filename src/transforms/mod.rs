//! Request/response transformations for the Anthropic API proxy.
//!
//! This module provides:
//! - `prepare`: native bypass and policy-driven request preparation
//! - `billing`: captured prompt markers and final serialized-body checksum
//! - `cache_policy`: preserve explicit TTLs or add bounded cache breakpoints
//! - `tool_adapters` / `tool_aliases`: schema-checked, reversible tool contracts
//! - `tool_events`: restore adapted arguments once streamed JSON is complete
//! - `openai_compat` / `responses`: API format conversion
//! - `streaming`: SSE format conversion and usage accounting

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
