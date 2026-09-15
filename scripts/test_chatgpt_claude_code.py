"""Exercise the installed Claude Code client against the deployed GPT Messages route."""
import argparse
import json
from pathlib import Path
import os
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--model', default='gpt-5.6-terra')
parser.add_argument('--read-file', action='store_true', help='Exercise Read on a temporary Markdown file')
args = parser.parse_args()
env = os.environ.copy()
env.update(
    ANTHROPIC_BASE_URL=os.environ['PROXY_BASE_URL'].rstrip('/'),
    ANTHROPIC_API_KEY=os.environ['PROXY_API_KEY'],
    ANTHROPIC_AUTH_TOKEN=os.environ['PROXY_API_KEY'],
    CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC='1',
)
# Keep the normal wire request with isolated project/settings context.
with tempfile.TemporaryDirectory(prefix='relay-live-cli-') as cwd:
    command = [
        'claude', '--setting-sources', '', '--strict-mcp-config', '--mcp-config',
        '{"mcpServers":{}}', '--no-session-persistence', '--disable-slash-commands',
        '--system-prompt', 'You are a test assistant.', '--model', args.model,
        '--effort', 'high',
    ]
    if args.read_file:
        Path(cwd, 'README.md').write_text('Verification phrase: READ_TOOL_OK_71842\n')
        command += ['--allowedTools', 'Read', '--max-turns', '3', '--output-format',
                    'stream-json', '--verbose', '-p',
                    'Use Read to read README.md and reply with its verification phrase.']
    else:
        command += ['--max-turns', '1', '-p', 'Reply exactly OK without using tools.']
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=120)
print('Claude Code exit:', result.returncode)
assert result.returncode == 0, result.stdout[:1000]
if args.read_file:
    events = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    blocks = [block for event in events for block in event.get('message', {}).get('content', []) if isinstance(block, dict)]
    reads = [block for block in blocks if block.get('type') == 'tool_use' and block.get('name') == 'Read']
    assert reads, 'Read was not called'
    assert all('pages' not in block['input'] for block in reads), reads
    assert not any(block.get('is_error') for block in blocks if block.get('type') == 'tool_result'), 'Tool execution failed'
    assert any(event.get('type') == 'result' and 'READ_TOOL_OK_71842' in event.get('result', '') for event in events), 'File content not returned'
    print('Read executed successfully without pages or tool errors')
else:
    print(result.stdout.strip())
    assert result.stdout.strip() == 'OK', 'Claude Code smoke test failed'
