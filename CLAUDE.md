# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Build & Development Commands

```bash
just run              # Run dev server (cargo run)
just build            # Build release binary (builds admin UI first, then cargo build --release)
just build-ui         # Build admin UI only (vp install + vue-tsc + vp build)
just test             # Run Rust unit tests (cargo test)
just check            # Full check: cargo fmt --check, clippy -D warnings, cargo test, vp check
just fmt              # Format Rust code
just lint             # Clippy + vp lint
just sqlx-prepare     # Regenerate committed sqlx query metadata after SQL changes
just openapi          # Regenerate OpenAPI TypeScript client (no running backend needed)
just test-openai      # Run OpenAI compatibility test (uv run with openai SDK)
just test-anthropic   # Run Anthropic native API test (uv run with anthropic SDK)
just test-api         # Run both integration tests against running proxy
just deploy           # Build release + deploy to server (rsync + systemctl restart)
just logs             # Tail server logs (journalctl -f)
just status           # Check server systemd status
just restart          # Restart server service
```

## Architecture

Unified API proxy that lets AI coding assistants (Cline, Roo Code, etc.) use a Claude Pro/Max subscription via either **OpenAI-compatible** or **Anthropic native** API formats.

**Stack**: Axum web framework, PostgreSQL via sqlx, reqwest HTTP client, tokio async runtime. Rust 2024 edition.

**Admin UI**: Vue 3 + TypeScript SPA in `admin-ui/`, using [Vite+](https://viteplus.dev/) as the unified toolchain. The build output (`admin-ui/dist/`) is embedded into the binary at compile time via `build.rs` using `memory-serve`. See `admin-ui/AGENTS.md` for Vite+ workflow details.

**Database**: PostgreSQL. Configure with `CLAUDE_PROXY_DATABASE_URL` or `DATABASE_URL`. A global `sqlx::PgPool` is initialized via `OnceCell` in `src/db.rs`.

SQL must use `sqlx::query!`, `query_as!`, or `query_scalar!` macros so queries are checked at compile time. Keep `.sqlx/` committed; run `just sqlx-prepare` with `DATABASE_URL` pointed at a PostgreSQL schema matching `src/db.rs` whenever SQL changes.

### Database Schema

Schema setup is managed in `src/db.rs`. Key points:

- On startup, `create_current_schema()` ensures all current PostgreSQL tables and indexes exist
- `seed_models_if_empty()` inserts default model pricing when the `models` table is empty
- Schema changes should be applied explicitly when needed

## Module Structure

- **`src/main.rs`** — Entry point, `AppState` struct, route setup, admin auth middleware (session cookies + Basic Auth with constant-time comparison)
- **`src/routes/`** — HTTP handlers:
  - `openai.rs` — `POST /v1/chat/completions`, `GET /v1/models`
  - `anthropic.rs` — `POST /v1/messages`, `POST /v1/messages/count_tokens`
  - `admin.rs` — OAuth management, API key CRUD, session auth, static file serving
  - `auth.rs` — Request authentication helpers, builds Anthropic request with OAuth headers
  - `health.rs` — `GET /health`
- **`src/transforms/`** — Request/response conversion pipeline:
  - `prepare.rs` — Unified pipeline: extract betas, inject fake user ID, add `mcp_` tool prefix, inject system message, auto-inject cache control breakpoints
  - `openai_compat.rs` — OpenAI format <-> Anthropic format conversion, thinking/reasoning support via model suffixes like `claude-sonnet-4-5(high)`
  - `streaming.rs` — SSE stream transformation (Anthropic->OpenAI), usage tracking during streams, 15s keep-alive pings
  - `tool_names.rs` — Add/strip `mcp_` prefix on tool names (required by OAuth)
  - `common.rs` — Fake user ID generation, cache control injection (max 4 breakpoints per request)
- **`src/auth/`** — Authentication and authorization:
  - `oauth.rs` — Anthropic OAuth 2.0 with PKCE, token refresh
  - `client_keys.rs` — API key generation (`sk-proxy-*`), validation, per-key rate limiting (hourly/weekly/total), per-model usage tracking, per-key model access control
  - `models.rs` — Dynamic model management (CRUD), pricing (input/output/cache_read/cache_write in $/MTok)
  - `storage.rs` — Auth credential persistence in PostgreSQL
  - `usage.rs` — Token usage tracking, cost calculation in microdollars (1 USD = 1,000,000 microdollars)
- **`src/config.rs`** — Environment variable config (`CLAUDE_PROXY_HOST`, `CLAUDE_PROXY_PORT`, `CLAUDE_PROXY_DATABASE_URL`/`DATABASE_URL`, `CLAUDE_PROXY_ADMIN_USERNAME`, `CLAUDE_PROXY_ADMIN_PASSWORD`, `CLAUDE_PROXY_CORS_ORIGINS`)
- **`src/constants.rs`** — API URLs, seed model list with pricing, beta headers, output token limits
- **`src/db.rs`** — PostgreSQL initialization and current schema setup
- **`src/error.rs`** — `ProxyError` enum with OpenAI and Anthropic error response formats

## Frontend (`admin-ui/`)

Vue 3 + TypeScript SPA using Nuxt UI v4, Tailwind CSS v4, and Vite+ (unified toolchain). Uses `@hey-api/openapi-ts` to auto-generate a typed API client from the backend's OpenAPI spec.

```bash
cd admin-ui && vp install        # Install dependencies
cd admin-ui && vp dev            # Dev server on port 5173 (proxies API to backend)
cd admin-ui && vp exec vue-tsc --build && vp build  # Production build
cd admin-ui && vp check          # Format + lint + type checks
cd admin-ui && vp lint           # Lint only
cd admin-ui && vp fmt src/       # Format only
```

- `src/client/` — **Auto-generated** OpenAPI TypeScript client. **NEVER edit files in this directory manually.** Always regenerate with `just openapi` while the backend is running
- `src/views/` — Page components
- `src/components/` — Reusable components
- `src/composables/` — Vue composables
- `src/router/` — Vue Router config

## Important Rules

- **NEVER start the backend server directly** (e.g. `cargo run &`, `just run &`). When the backend needs to be running (for integration tests, etc.), use the Bash tool's `run_in_background` parameter to start it, then ask the user to tell you when it's ready. Never use shell `&` backgrounding.
- **NEVER manually edit files in `admin-ui/src/client/`**. These are auto-generated. Always regenerate with `just openapi` (no running backend needed — uses `--openapi` flag to dump the spec from code annotations).

## Key Patterns

- **Shared state**: `AppState` passed via Axum's `State<Arc<AppState>>` containing auth stores, HTTP client, admin sessions
- **Request pipeline**: All requests go through `prepare_anthropic_request()` in `transforms/prepare.rs` which applies transforms in sequence
- **Dual format support**: OpenAI requests are converted to Anthropic format, sent upstream, then responses converted back. Anthropic requests pass through with minimal transforms
- **Security**: All credential/key comparisons use `subtle::ConstantTimeEq` to prevent timing attacks
- **Streaming**: Uses `async-stream` with `tokio::select!` biased for data over keep-alive pings; usage is accumulated from SSE events and recorded after stream ends
