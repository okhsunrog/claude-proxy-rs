/// Anthropic API URL for messages endpoint (with beta features)
pub const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages?beta=true";

/// Anthropic API URL for token counting (with beta features)
pub const ANTHROPIC_COUNT_TOKENS_URL: &str =
    "https://api.anthropic.com/v1/messages/count_tokens?beta=true";

/// Anthropic API URL for subscription usage (OAuth)
pub const ANTHROPIC_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";

/// Anthropic API URL for OAuth profile (plan detection)
pub const ANTHROPIC_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";

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

/// Seed models for initial database population.
/// Used only on first startup when the models table is empty.
/// After that, models are managed via the admin UI.
/// Format: (id, input_price, output_price, cache_read_price, cache_write_price) — all $/MTok
pub static SEED_MODELS: &[(&str, f64, f64, f64, f64)] = &[
    ("claude-opus-4-6", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-5-20251101", 5.0, 25.0, 0.50, 6.25),
    ("claude-opus-4-5", 5.0, 25.0, 0.50, 6.25),
    ("claude-sonnet-4-6", 3.0, 15.0, 0.30, 3.75),
    ("claude-sonnet-4-5-20250929", 3.0, 15.0, 0.30, 3.75),
    ("claude-sonnet-4-5", 3.0, 15.0, 0.30, 3.75),
    ("claude-haiku-4-5-20251001", 1.0, 5.0, 0.10, 1.25),
    ("claude-haiku-4-5", 1.0, 5.0, 0.10, 1.25),
    ("claude-opus-4-1-20250805", 15.0, 75.0, 1.50, 18.75),
    ("claude-opus-4-1", 15.0, 75.0, 1.50, 18.75),
    ("claude-opus-4-20250514", 15.0, 75.0, 1.50, 18.75),
    ("claude-opus-4-0", 15.0, 75.0, 1.50, 18.75),
    ("claude-sonnet-4-20250514", 3.0, 15.0, 0.30, 3.75),
    ("claude-sonnet-4-0", 3.0, 15.0, 0.30, 3.75),
];
