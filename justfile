set dotenv-load

# Use clang 18 for building (clang 22 breaks aegis crate's AVX-512 code)
export PATH := "/usr/lib/llvm18/bin:" + env("PATH")
export SQLX_OFFLINE := "true"

server := "root@" + env("CLAUDE_PROXY_DEPLOY_HOST", "sg.okhsunrog.dev")
cloud_forge := env("CLOUD_FORGE_DIR", env("HOME") + "/code/cloud-forge")

# Run the proxy server
run:
    cargo run

# Build admin UI
build-ui:
    cd admin-ui && vp install && vp exec vue-tsc --build && vp build

# Build release binary (includes embedded UI)
build: build-ui
    cargo build --release

# Portable Linux release, including on NixOS build hosts
build-portable: build-ui
    nix shell nixpkgs#cargo-zigbuild nixpkgs#zig nixpkgs#gnumake --command cargo zigbuild --locked --release --target x86_64-unknown-linux-musl

# Run cargo tests
test:
    cargo test

# Run all checks (fmt, clippy, tests, frontend)
check:
    cargo fmt --check
    cargo clippy -- -D warnings
    cargo test
    cd admin-ui && vp check

# Format code
fmt:
    cargo fmt

# Lint
lint:
    cargo clippy -- -D warnings
    cd admin-ui && vp lint

# Regenerate sqlx compile-time query metadata after changing SQL.
sqlx-prepare:
    SQLX_OFFLINE=false cargo sqlx migrate run
    SQLX_OFFLINE=false cargo sqlx prepare -- --all-targets

# Check committed sqlx metadata against DATABASE_URL.
sqlx-check:
    SQLX_OFFLINE=false cargo sqlx migrate run
    SQLX_OFFLINE=false cargo sqlx prepare --check -- --all-targets

# Regenerate OpenAPI TypeScript client (no running backend needed)
openapi:
    cargo run -- --openapi > admin-ui/openapi.json
    cd admin-ui && vp fmt openapi.json && vp exec openapi-ts

# Run OpenAI compatibility test
test-openai:
    uv run --with openai test_openai.py

# Run Anthropic native API test
test-anthropic:
    uv run --with anthropic test_anthropic.py

# Run integration tests against running proxy
test-api: test-openai test-anthropic

# Update the existing Singapore deployment; preserve its protected config
deploy: build-portable
    ANSIBLE_CONFIG="{{cloud_forge}}/ansible.cfg" CLAUDE_PROXY_RELEASE_BINARY="{{justfile_directory()}}/target/x86_64-unknown-linux-musl/release/claude-proxy-rs" uv run --project "{{cloud_forge}}" ansible-playbook -i "{{cloud_forge}}/inventory.yml" "{{cloud_forge}}/site-claude-proxy.yml" --limit singapore -e claude_proxy_start=true

# View server logs
logs:
    ssh {{server}} "journalctl -u claude-proxy -f"

# Check server status
status:
    ssh {{server}} "systemctl status claude-proxy --no-pager"

# Restart server
restart:
    ssh {{server}} "systemctl restart claude-proxy"
