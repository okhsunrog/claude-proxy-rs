"""Exercise the installed Claude Code client against the deployed GPT Messages route."""
import argparse
import os
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--model', default='gpt-5.6-terra')
args = parser.parse_args()
env = os.environ.copy()
env.update(
    ANTHROPIC_BASE_URL=os.environ['PROXY_BASE_URL'].rstrip('/'),
    ANTHROPIC_API_KEY=os.environ['PROXY_API_KEY'],
    ANTHROPIC_AUTH_TOKEN=os.environ['PROXY_API_KEY'],
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1',
)
# Keep the normal wire request and tool declarations, with isolated project/settings
# context and no persistent session. The test prompt requires no tool execution.
with tempfile.TemporaryDirectory(prefix='relay-live-cli-') as cwd:
    result = subprocess.run(
        ['claude', '--setting-sources', '', '--strict-mcp-config', '--mcp-config',
         '{"mcpServers":{}}', '--no-session-persistence', '--disable-slash-commands',
         '--system-prompt', 'You are a test assistant.', '--model', args.model,
         '--effort', 'high', '--max-turns', '1', '-p',
         'Reply exactly OK without using tools.'],
        cwd=cwd, env=env, capture_output=True, text=True, timeout=120,
    )
print('Claude Code exit:', result.returncode)
print(result.stdout.strip())
assert result.returncode == 0 and result.stdout.strip() == 'OK', 'Claude Code smoke test failed'
