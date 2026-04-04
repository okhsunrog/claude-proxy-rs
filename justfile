set dotenv-load

# Use clang 18 for building (clang 22 breaks aegis crate's AVX-512 code)
export PATH := "/usr/lib/llvm18/bin:" + env("PATH")

server := "root@" + env_var("HOME_SRV_IP")
deploy_dir := "/opt/claude-proxy"

# Run the proxy server
run:
    cargo run

# Build admin UI
build-ui:
    cd admin-ui && vp install && vp exec vue-tsc --build && vp build

# Build release binary (includes embedded UI)
build: build-ui
    cargo build --release

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

# Regenerate OpenAPI TypeScript client (no running backend needed)
openapi:
    cargo run -- --openapi > admin-ui/openapi.json
    cd admin-ui && vp exec openapi-ts

# Run OpenAI compatibility test
test-openai:
    uv run --with openai test_openai.py

# Run Anthropic native API test
test-anthropic:
    uv run --with anthropic test_anthropic.py

# Run integration tests against running proxy
test-api: test-openai test-anthropic

# Deploy to server
deploy: build
    @echo "=== Deploying to {{server}} ==="
    ssh {{server}} "mkdir -p {{deploy_dir}}"
    ssh {{server}} "test -f {{deploy_dir}}/.env || printf 'CLAUDE_PROXY_ADMIN_USERNAME=admin\nCLAUDE_PROXY_ADMIN_PASSWORD=changeme\n' > {{deploy_dir}}/.env && chmod 600 {{deploy_dir}}/.env"
    rsync -avz --progress target/release/claude-proxy-rs {{server}}:{{deploy_dir}}/
    scp claude-proxy.service {{server}}:/etc/systemd/system/
    ssh {{server}} "systemctl daemon-reload && systemctl enable claude-proxy && systemctl restart claude-proxy"
    @sleep 2
    ssh {{server}} "systemctl status claude-proxy --no-pager"
    @echo ""
    @echo "=== Deployed! ==="
    @echo "Admin UI: http://mira.local:4096/admin"

# View server logs
logs:
    ssh {{server}} "journalctl -u claude-proxy -f"

# Check server status
status:
    ssh {{server}} "systemctl status claude-proxy --no-pager"

# Restart server
restart:
    ssh {{server}} "systemctl restart claude-proxy"
