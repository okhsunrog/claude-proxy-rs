//! Error type for the usage fetcher chain.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum FetchError {
    /// The fetcher has no credentials and can't be used. Signal to the
    /// chain to try the next fetcher without logging a warning.
    #[error("not configured")]
    NotConfigured,
    /// Network-level failure (DNS, TCP, TLS, timeout).
    #[error("network error: {0}")]
    Network(String),
    /// Got an HTTP response but the status was not 2xx.
    #[error("upstream returned {status}: {body}")]
    Upstream { status: u16, body: String },
    /// Response body could not be parsed as the expected JSON shape.
    #[error("failed to parse response: {0}")]
    Parse(String),
    /// Internal state error (e.g. DB read failed).
    #[error("internal: {0}")]
    Internal(String),
}
