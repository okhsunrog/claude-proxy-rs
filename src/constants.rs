/// Anthropic API URL for messages endpoint (with beta features)
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// Anthropic API URL for token counting (with beta features)
pub const ANTHROPIC_COUNT_TOKENS_URL: &str =
    "https://api.anthropic.com/v1/messages/count_tokens?beta=true";

/// Anthropic API version header value
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// OAuth beta features header value (matches Claude Code 2.1.32)
/// Includes adaptive-thinking for Opus 4.6 support
pub const OAUTH_BETA_HEADER: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,fine-grained-tool-streaming-2025-05-14,prompt-caching-scope-2026-01-05,adaptive-thinking-2026-01-28";

/// Max output tokens for Opus 4.6 (128K)
pub const OPUS_4_6_MAX_OUTPUT: u32 = 128000;

/// Default max output tokens for Claude 4 models (64K)  
pub const DEFAULT_MAX_OUTPUT: u32 = 64000;

/// User agent string for OAuth requests (mimics Claude CLI)
pub const USER_AGENT: &str = "claude-cli/2.1.32 (external, cli)";

/// System message prefix for OAuth requests (Claude Code identity)
pub const SYSTEM_PREFIX: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Available Claude models
pub static MODELS: &[&str] = &[
    "claude-opus-4-6",
    "claude-opus-4-5-20251101",
    "claude-opus-4-5",
    "claude-sonnet-4-5-20250929",
    "claude-sonnet-4-5",
    "claude-haiku-4-5-20251001",
    "claude-haiku-4-5",
    "claude-opus-4-1-20250805",
    "claude-opus-4-1",
    "claude-opus-4-20250514",
    "claude-opus-4-0",
    "claude-sonnet-4-20250514",
    "claude-sonnet-4-0",
];
