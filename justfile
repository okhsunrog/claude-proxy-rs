set dotenv-load

server := "root@mira.local"
deploy_dir := "/opt/claude-proxy"

# Run the proxy server
run:
    cargo run

# Build admin UI
build-ui:
    cd admin-ui && bun install && bun run build

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
    cd admin-ui && bun run type-check && bun run lint

# Format code
fmt:
    cargo fmt

# Lint
lint:
    cargo clippy -- -D warnings
    cd admin-ui && bun run lint

# Regenerate OpenAPI TypeScript client (backend must be running)
openapi:
    cd admin-ui && bun run openapi-ts

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
