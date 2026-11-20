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

/// Beta header for usage/profile API calls — only the OAuth beta, matching Claude Code exactly.
pub const OAUTH_USAGE_BETA: &str = "oauth-2025-04-20";

/// Base beta header for inference requests — the stable core common to Claude
/// Code 2.1.326 local mock captures, plus `oauth-2025-04-20` (required for the
/// OAuth-token upstream, also sent by current native Claude Code).
/// Per-request, client-specific betas (context-1m, advisor-tool,
/// mid-conversation-system, thinking-token-count, …) are forwarded from the
/// inbound `anthropic-beta` header and merged on top of this base.
pub const OAUTH_BETA_HEADER: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,thinking-token-count-2026-05-13,effort-2025-11-24,extended-cache-ttl-2025-04-11";

/// First-party defaults observed in the macOS 2.1.326 OAuth capture matrix.
pub const CURRENT_INFERENCE_BETAS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,tool-provenance-2026-10-26,thinking-token-count-2026-05-13,effort-2025-11-24,thinking-binding-controls-2026-08-01,extended-cache-ttl-2025-04-11,cache-diagnosis-2026-04-07";
pub const CURRENT_HAIKU_BETAS: &str = "oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,claude-code-20250219,tool-provenance-2026-10-26,thinking-token-count-2026-05-13,thinking-binding-controls-2026-08-01,extended-cache-ttl-2025-04-11,cache-diagnosis-2026-04-07";

/// Max output tokens for Opus 4.6 (128K)
pub const OPUS_4_6_MAX_OUTPUT: u32 = 128000;

/// Default max output tokens for Claude 4 models (64K)  
pub const DEFAULT_MAX_OUTPUT: u32 = 64000;

/// Version of the captured client, not a claim about the latest upstream release.
/// Keep the user-agent literals below and capture fixtures in sync when upgrading.
pub const CAPTURED_CLAUDE_CODE_VERSION: &str = "2.1.326";

/// User-Agent for generated inference requests, observed in SDK-style captures.
/// Format: claude-cli/{VERSION} ({USER_TYPE}, {ENTRYPOINT})
pub const INFERENCE_USER_AGENT: &str = "claude-cli/2.1.326 (external, sdk-cli)";

/// Generated client identity. Native clients retain their own platform headers.
pub const INFERENCE_OS: &str = "MacOS";
pub const INFERENCE_ARCH: &str = "x64";

/// User-Agent for admin/non-inference requests, separate from the inference UA.
/// Used for usage API, profile, bootstrap calls.
pub const USER_AGENT: &str = "claude-code/2.1.326";

/// System message prefix for OAuth requests (Claude Code identity)
pub const SYSTEM_PREFIX: &str = "You are a Claude agent, built on Anthropic's Claude Agent SDK.";

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
