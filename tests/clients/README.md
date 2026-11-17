The ignored `real_clients_through_http_and_postgres` Rust test starts the production Messages and Chat Completions handlers, backed by an isolated PostgreSQL database, and a deterministic local upstream. It runs the real Python SDKs with both response modes. The clients read a file, report a missing-file error, edit the file, continue the conversation and cancel a separate stream. The test checks translated upstream schemas/history and exact usage totals in PostgreSQL, including cancellation. No paid API calls or real OAuth credentials are involved.

Run from the repository root with an empty disposable database named `proxy_compat` on loopback:

```sh
uv sync --project tests/clients
PROXY_AUDIT_DATABASE_URL=postgresql://postgres:password@127.0.0.1:54439/proxy_compat \
PROXY_AUDIT_ROOT=/home/okhsunrog/tmp_zfs/codex_tmp/proxy-compat-audit \
cargo test real_clients_through_http_and_postgres -- --ignored --nocapture
```

The test applies migrations and creates keys/auth rows. Its endpoint injection is internal application state; normal startup always uses the production API constants. It saves synthetic upstream requests beneath `PROXY_AUDIT_ROOT`. This verifies SDK tool loops, not execution inside the Cline or Roo extension host.
