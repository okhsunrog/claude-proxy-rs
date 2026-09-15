# claude-proxy-rs

Unified API proxy for Claude Pro/Max subscription.

Use your existing Claude Pro/Max subscription with AI coding assistants and tools that support either **OpenAI-compatible** or **Anthropic native** APIs, including [Cline](https://cline.bot/), [Roo Code](https://roocode.com/), [Kilo Code](https://kilo.ai/), and more.

## Admin UI

| Overview | API Keys | Models |
|:---:|:---:|:---:|
| ![Admin Overview](docs/admin-overview.png) | ![API Key Details](docs/admin-keys.png) | ![Models](docs/admin-models.png) |

## Features

- **Dual API support:** OpenAI-compatible (`/v1/chat/completions`) and Anthropic native (`/v1/messages`)
- OAuth authentication with Claude Pro/Max subscription
- Admin UI (Vue 3 SPA) for managing OAuth, API keys, models, and usage
- Streaming support with keep-alive pings (prevents timeouts during extended thinking)
- Tool/function calling, image inputs (base64)
- Extended thinking mode (configurable via model suffix or native API parameters)
- Automatic prompt caching (auto-injects cache breakpoints for tools, system, and conversation history)
- Token counting (`/v1/messages/count_tokens`)
- **Per-key cost-based rate limiting** (5-hour/weekly/total limits in USD, synced with subscription windows)
- **Per-key model access control** (allow all or whitelist specific models)
- **Per-model usage tracking** with cost calculation (input/output/cache pricing)
- **Usage history** — time-series charts for cost and tokens, breakdowns by model and API key (24h/7d/30d)
- **User-facing usage dashboard** at `/admin/usage` — no admin auth required, authenticate with your `sk-proxy-*` key
- **Dynamic model management** (add/remove models, configure per-token pricing)
- Key enable/disable toggle
- Configurable cloaking mode (`always`/`never`/`auto`)
- Single binary deployment (admin UI embedded via memory-serve)

---

## Installation

### Requirements

- [Rust](https://www.rust-lang.org/) >= 1.94.0 (edition 2024)
- [Bun](https://bun.sh/) >= 1.3.0
- [Vite+](https://viteplus.dev/) (`vp` CLI) — unified frontend toolchain
- [just](https://github.com/casey/just) command runner
- PostgreSQL

### Build & run

Create a PostgreSQL database and user:

```bash
sudo -u postgres createuser --pwprompt claude_proxy
sudo -u postgres createdb -O claude_proxy claude_proxy
```

```bash
# Create .env with required admin credentials
cp .env.example .env
# Edit .env with your credentials and PostgreSQL URL

just build   # builds admin UI + release binary
just run     # or: cargo run
```

Open http://127.0.0.1:4096/admin, log in, connect your Claude account via OAuth, and generate API keys (`sk-proxy-*`).

### Configuration

Environment variables are loaded from `.env` or the environment.

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDE_PROXY_ADMIN_USERNAME` | *(required)* | Admin username |
| `CLAUDE_PROXY_ADMIN_PASSWORD` | *(required)* | Admin password |
| `CLAUDE_PROXY_DATABASE_URL` / `DATABASE_URL` | *(required)* | PostgreSQL connection URL |
| `CLAUDE_PROXY_HOST` | `127.0.0.1` | Bind address |
| `CLAUDE_PROXY_PORT` | `4096` | Port |
| `CLAUDE_PROXY_CORS_ORIGINS` | `localhost` | CORS: `localhost`, `*`, or comma-separated origins |
| `CLAUDE_PROXY_CLOAK_MODE` | `auto` | Cloaking: `always`, `never`, `auto` (skips cloaking for Claude Code clients) |
| `CLAUDE_PROXY_CAPTURE_DIR` | *(unset)* | Optional directory for redacted request/response captures. Enable only for debugging. |

Admin sessions use HttpOnly cookies with a 30-day sliding expiration. Basic Auth is also accepted.

### Request captures

Set `CLAUDE_PROXY_CAPTURE_DIR=/path/to/captures` to write one directory per inference request. Captures include redacted client headers, inbound JSON, prepared Anthropic JSON, upstream response headers, and either `upstream_body.txt` or raw `upstream_stream.sse` chunks.

Capture files may contain prompts, tool results, code, and model outputs. API keys, authorization headers, and cookies are redacted from headers, but the capture directory should still be treated as sensitive.

### Data storage

All data (OAuth credentials, API keys, usage) is stored in PostgreSQL. Configure the connection with `CLAUDE_PROXY_DATABASE_URL` or `DATABASE_URL`.

SQL queries use `sqlx::query!`/`query_as!` compile-time checks. The generated `.sqlx/` metadata is committed so normal builds and CI do not need database access. After changing SQL, run this with `DATABASE_URL` pointing at a PostgreSQL schema matching `migrations/`:

```bash
just sqlx-prepare
```

---

## Deployment

The repository includes a systemd unit (`claude-proxy.service`) and a `just deploy` recipe. The recipe builds the embedded admin UI, builds the release binary, copies it to the server, installs the systemd unit, restarts the service, and checks `/health`.

### Server prerequisites

Install PostgreSQL on the server and create a dedicated database/user:

```bash
sudo -u postgres createuser --pwprompt claude_proxy
sudo -u postgres createdb -O claude_proxy claude_proxy
```

Create `/opt/claude-proxy/.env` on the server:

```bash
sudo mkdir -p /opt/claude-proxy
sudo install -m 600 /dev/null /opt/claude-proxy/.env
sudoedit /opt/claude-proxy/.env
```

Required server environment:

```dotenv
CLAUDE_PROXY_ADMIN_USERNAME=admin
CLAUDE_PROXY_ADMIN_PASSWORD=change-this
CLAUDE_PROXY_DATABASE_URL=postgresql://claude_proxy:change-this@127.0.0.1:5432/claude_proxy
```

The service binds to `0.0.0.0:4096` via `claude-proxy.service`.

### Deploy from this repo

Set the target host used by the `justfile`:

```bash
export HOME_SRV_IP=10.77.77.100
```

Then deploy:

```bash
just deploy
```

Useful server commands:

```bash
just status
just logs
just restart
```

Manual equivalent:

```bash
just build
rsync -avz target/release/claude-proxy-rs root@$HOME_SRV_IP:/opt/claude-proxy/
scp claude-proxy.service root@$HOME_SRV_IP:/etc/systemd/system/
ssh root@$HOME_SRV_IP "systemctl daemon-reload && systemctl enable claude-proxy && systemctl restart claude-proxy"
ssh root@$HOME_SRV_IP "curl -fsS http://127.0.0.1:4096/health"
```

Open `http://<server>:4096/admin`, log in, connect OAuth, and create `sk-proxy-*` API keys.

---

## Usage

### OpenAI-Compatible API

```python
from openai import OpenAI

client = OpenAI(base_url="http://127.0.0.1:4096/v1", api_key="sk-proxy-...")

response = client.chat.completions.create(
    model="claude-sonnet-4-6",
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.choices[0].message.content)
```

### Anthropic Native API

```python
from anthropic import Anthropic

client = Anthropic(base_url="http://127.0.0.1:4096", api_key="sk-proxy-...")

response = client.messages.create(
    model="claude-sonnet-4-6",
    max_tokens=1024,
    messages=[{"role": "user", "content": "Hello!"}]
)
print(response.content[0].text)
```

Both APIs accept `x-api-key: sk-proxy-...` or `Authorization: Bearer sk-proxy-...`.

### IDE Extensions

#### Cline / Roo Code / Kilo Code (Recommended)

Use the native **Anthropic** provider:

| Setting | Value |
|---------|-------|
| API Provider | `Anthropic` |
| API Key | Your `sk-proxy-...` key |
| Use custom base URL | ✓ Enabled |
| Base URL | `http://127.0.0.1:4096` |
| Model | `claude-sonnet-4-6` (or any model) |

Extended thinking works via the extension's built-in controls — no model suffixes needed.

#### Custom Tool Names

Claude subscription OAuth is sensitive to the tool names sent to the native Anthropic API. Claude Code uses a fixed set of `mcp_*` tool names; third-party tools often send their own names such as `shell`, `patch`, `ask_followup_question`, or `update_todo_list`.

In cloaking mode, the proxy best-effort normalizes known third-party tool names to Claude Code-compatible names before forwarding the request upstream. Tool descriptions and input schemas are kept intact, and response tool calls are mapped back to the client's original names before returning to the tool.

Unknown or colliding tool names fail locally with `400 Bad Request` instead of being forwarded upstream. This is intentional: new tool vocabularies should be captured, reviewed, and added explicitly.

Current tested support:

| Tool | API mode tested | Status | Notes |
|------|-----------------|--------|-------|
| Claude Code | Anthropic native | Supported | Treated as first-party shape; cloaking is skipped in `auto` mode. |
| ForgeCode | Anthropic native | Supported | Tested with tool normalization for `fs_search`, `Read`, `Write`, `undo`, `remove`, `patch`, `multi_patch`, `shell`, `fetch`, `skill`, `todo_write`, `todo_read`, and `Task`. |
| Roo Code | Anthropic native | Supported | Tested with tool normalization for `ask_followup_question`, `attempt_completion`, `codebase_search`, `list_files`, `new_task`, `read_file`, `skill`, `search_files`, `switch_mode`, and `update_todo_list`. |
| Kilo Code | Anthropic native | Not verified | Expected to work if it uses a supported tool vocabulary; capture and add aliases if it returns a local unknown-tool `400`. |
| Cline | Anthropic native | Not recently verified | Basic native Anthropic shape should work; custom tool names may need aliases. |

Some aliases are semantically approximate because Anthropic only accepts the Claude Code-compatible name set. The model still sees the original tool description and schema, which are the primary signals for how to call the tool.

#### Alternative: OpenAI Compatible

| Setting | Value |
|---------|-------|
| API Provider | `OpenAI Compatible` |
| Base URL | `http://127.0.0.1:4096/v1` |
| API Key | Your `sk-proxy-...` key |
| Model ID | `claude-sonnet-4-5(high)` |

**Extended thinking suffixes** (OpenAI mode only):

**Opus 4.6** — adaptive thinking with effort parameter:
| Suffix | Effort |
|--------|--------|
| `(low)` | `low` |
| `(medium)` | `medium` |
| `(high)` | `high` |
| `(xhigh)` / `(max)` | `max` |

**Older models** (Sonnet 4.5, Opus 4.5, etc.) — manual thinking with budget_tokens:
| Suffix | Budget Tokens |
|--------|---------------|
| `(low)` | 1,024 |
| `(medium)` | 8,192 |
| `(high)` | 32,000 |
| `(xhigh)` | 64,000 |
| `(16000)` | Custom value |

### Available Models

- `claude-opus-4-6`, `claude-sonnet-4-6` (latest, adaptive thinking)
- `claude-opus-4-5`, `claude-sonnet-4-5`, `claude-haiku-4-5`
- `claude-opus-4-1`, `claude-opus-4-0`, `claude-sonnet-4-0`

### API Endpoints

**OpenAI-Compatible**
- `POST /v1/chat/completions` — streaming supported
- `GET /v1/models`

Response extensions (ignored by standard clients):

| Field | Location | Description |
|-------|----------|-------------|
| `reasoning_content` | `choices[].message` | Extended thinking output |
| `cache_creation_input_tokens` | `usage` | Tokens written to prompt cache |
| `cache_read_input_tokens` | `usage` | Tokens read from prompt cache |

Request extensions:

| Field | Description |
|-------|-------------|
| `reasoning_effort` | `low`/`medium`/`high`/`max` — alternative to model suffix |

**Anthropic Native**
- `POST /v1/messages` — streaming supported
- `POST /v1/messages/count_tokens`
- `GET /v1/models`

**Admin**
- `GET /admin` — Admin UI
- `GET /admin/usage` — User-facing usage dashboard (Bearer key auth)

**Health**
- `GET /health`

---

## Development

```bash
just run                  # Start backend (cargo run)
```

For frontend hot reload, run in a second terminal:
```bash
cd admin-ui && vp install && vp dev
# Open http://localhost:5173/admin/
```

### Regenerating the API client

The TypeScript client in `admin-ui/src/client/` is auto-generated from the OpenAPI spec. After changing backend routes or types, regenerate it with:

```bash
just openapi   # dumps spec via --openapi flag (no running server needed), then regenerates client
```

### All recipes

```bash
just check    # fmt + clippy + tests + frontend checks
just build    # build UI + release binary
just fmt      # format Rust code
just lint     # clippy + frontend lint
just test     # Rust unit tests
just openapi  # regenerate TypeScript client
just deploy   # build + deploy to server
just logs     # tail server logs
just status   # server systemd status
just restart  # restart server service
just test-api # integration tests against running proxy
```

### ChatGPT file transcription

The admin panel can connect one ChatGPT account using device-code login. Enable
**Device code login** in your ChatGPT security settings, select **Connect ChatGPT**,
open the verification link, enter the displayed code, then select **Check sign-in**.
Credentials are stored separately from Claude in PostgreSQL and refreshed when
needed. Disconnect removes the proxy's stored credentials; it does not revoke
other sessions in ChatGPT.

`POST /v1/audio/transcriptions` accepts a proxy bearer key and multipart fields:

- `file`: one non-empty recording, at most 25 MiB. Recognized filename extensions:
  `ogg`, `webm`, `wav`, `mp3`, `m4a`, `mp4`, `flac`. OGG/Opus was verified live.
- `language`: optional language code, for example `ru`.
- `model`: optional; only `chatgpt-transcribe` is accepted.
- `response_format`: optional; only `json` is accepted.

The response is `{"text":"..."}`. Audio is forwarded to the desktop backend's
`/transcribe` endpoint. No cleanup/rewrite model is applied, and the upstream
speech-recognition model is not disclosed. This is an internal subscription
endpoint, so account availability and compatibility may change.

Transcription uses enabled proxy keys. Keys with a model whitelist must explicitly
allow `chatgpt-transcribe` (add that identifier in Models before assigning it).
It has separate in-memory limits: 10 attempts per minute per key, two simultaneous
uploads/requests per process, and a 60-second upstream timeout. These counters reset
on process restart. Claude subscription checks and monetary token budgets do not
apply: the transcription backend returns no token, cost, or quota usage. It is
therefore not included in token-cost history. Audio and transcripts are not saved
or captured by the proxy.

For a local verification using the existing subscription login:

```bash
uv run scripts/transcribe_probe.py /path/to/recording.ogg --direct
```

To test a running proxy instead, set `PROXY_API_KEY` in your environment and run:

```bash
uv run scripts/transcribe_probe.py /path/to/recording.ogg --base-url http://127.0.0.1:4096
```

The probe never prints credentials. `--output /path/to/transcript.txt` optionally
saves the returned text. Microphone recording belongs in the consuming client;
the admin panel manages the connection only.


### GPT inference through the ChatGPT subscription

The same connected account serves `POST /v1/responses` and GPT requests to
`POST /v1/chat/completions` and `POST /v1/messages`, with ordinary JSON and SSE streaming responses.
In the ChatGPT subscription card, select **Load available models** to query the
account's catalog. Register the desired IDs and prices in **Models**, then grant
access to restricted keys. Adding a model does not grant upstream entitlement.
IDs beginning with `gpt-`, `codex-`, or the `o1`/`o3`/`o4` families route to ChatGPT;
Claude models continue through the existing provider. GPT token counting through
`/v1/messages/count_tokens` is not supported.

Responses is the native interface and preserves output items, including opaque
reasoning items needed for conversation replay. Send the full conversation on
each request: storage, previous response IDs, background execution and conversation
IDs are not supported. Requests use `store=false`; the upstream is always streamed
and ordinary JSON responses are assembled from completed output items.

Chat Completions supports text, user images, function tools and tool results,
`tool_choice`, `parallel_tool_calls`, `reasoning_effort`, and structured output via
`response_format`. Streaming supports `stream_options.include_usage`. Only one
choice is supported. Translated function tools default to `strict: false` so
optional arguments remain optional; an explicit `strict: true` is preserved.

Messages supports text, images, inline documents, function tools/results, tool
choice, structured output and thinking. Signed GPT reasoning is carried in a
versioned `thinking.signature` envelope so returned assistant blocks can be
replayed unchanged. Foreign Claude signatures and native server tools cannot be
translated to GPT. Native Claude requests retain their existing preparation path.

GPT Chat Completions and Messages default to compatible mode. Unsupported
`max_tokens` / `max_completion_tokens` / `max_output_tokens`, `temperature`,
`top_p`, `top_k` and stop controls are ignored with an
`x-proxy-compatibility-warnings` response header. Consequently `max_tokens` does
**not** cap GPT generation. Thinking token budgets approximate reasoning effort;
Claude cache hints do not transfer. Messages `context_management` is ignored
with a warning: server-side context editing is unavailable and the full supplied
history is retained. Inline `role: system` messages from newer clients are moved
to system instructions with a `messages.system` warning. Send `x-proxy-compatibility: strict` to reject
these approximations with HTTP 400. Native Responses defaults to strict mode;
`x-proxy-compatibility: compatible` opts into the same control handling. Unsupported
content and stateful requests always fail. Model-specific upstream restrictions
still apply. Use Responses for native tools and native reasoning replay.

GPT usage is recorded in the existing request history and uses model prices for
estimated API-equivalent costs. Cache reads and writes are counted separately, without double
charging them as uncached input. Prices are static per model: long-context and
service-tier price multipliers are not applied automatically. Global and per-model key budgets remain shared
proxy budgets, with the existing common reset windows; they are not a report of
ChatGPT subscription allowance. Claude subscription exhaustion does not block GPT.
ChatGPT rate-limit errors are returned as HTTP 429. Usage is recorded only when the
upstream reports it; a cancellation before usage arrives cannot be charged accurately.
There is a 90-second event inactivity timeout, a 10-minute overall upstream timeout,
and one authentication retry on HTTP 401. Mid-stream failures emit an error event
without a successful completion marker. Opt-in request capture also covers GPT.

Example JSON request (use an ID available to your account):

```json
{"model":"gpt-5.3-codex-spark","input":"Reply with a short greeting.","stream":false}
```

The upstream SSE event format follows the [Responses streaming protocol](https://developers.openai.com/api/docs/guides/streaming-responses).

Run the live compatibility checks against your configured proxy:

```bash
uv run --env-file .env --with openai scripts/test_chatgpt.py --model YOUR_MODEL_ID
uv run --env-file .env --with anthropic scripts/test_chatgpt_messages.py --model YOUR_MODEL_ID
uv run --env-file .env scripts/test_chatgpt_claude_code.py --model YOUR_MODEL_ID
uv run --env-file .env scripts/test_chatgpt_claude_code.py --model YOUR_MODEL_ID --read-file
```
