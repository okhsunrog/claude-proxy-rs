"""Capture installed Claude Code traffic with dummy credentials and a local SSE mock.

Run: uv run scripts/capture_claude.py /absolute/path/to/capture-directory
Use a task directory under /home/okhsunrog/tmp_zfs/codex_tmp on this host.
Each case has an isolated HOME; authorization headers are never saved.
"""

import http.server
import json
import os
import pathlib
import shutil
import subprocess
import sys
import threading

root = pathlib.Path(sys.argv[1]).resolve()
root.mkdir(parents=True, exist_ok=True)


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        self.reply({"data": []})

    def reply(self, data):
        raw = json.dumps(data).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_POST(self):
        body = json.loads(
            self.rfile.read(int(self.headers.get("Content-Length", 0))) or "{}"
        )
        headers = {
            k.lower(): v
            for k, v in self.headers.items()
            if k.lower() not in ("authorization", "x-api-key")
        }
        captures.append({"path": self.path, "headers": headers, "body": body})
        if "/messages" not in self.path:
            return self.reply({})
        if "count_tokens" in self.path:
            return self.reply({"input_tokens": 10})
        message = {
            "id": "msg_mock",
            "type": "message",
            "role": "assistant",
            "model": body.get("model"),
            "content": [{"type": "text", "text": "OK"}],
            "stop_reason": "end_turn",
            "stop_sequence": None,
            "usage": {"input_tokens": 10, "output_tokens": 1},
        }
        if not body.get("stream"):
            return self.reply(message)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()
        events = [
            (
                "message_start",
                {
                    "type": "message_start",
                    "message": {**message, "content": [], "stop_reason": None},
                },
            ),
            (
                "content_block_start",
                {
                    "type": "content_block_start",
                    "index": 0,
                    "content_block": {"type": "text", "text": ""},
                },
            ),
            (
                "content_block_delta",
                {
                    "type": "content_block_delta",
                    "index": 0,
                    "delta": {"type": "text_delta", "text": "OK"},
                },
            ),
            ("content_block_stop", {"type": "content_block_stop", "index": 0}),
            (
                "message_delta",
                {
                    "type": "message_delta",
                    "delta": {"stop_reason": "end_turn", "stop_sequence": None},
                    "usage": {"output_tokens": 1},
                },
            ),
            ("message_stop", {"type": "message_stop"}),
        ]
        for event, data in events:
            self.wfile.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())


server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    for mode in ["key", "oauth"]:
        for model in ["claude-sonnet-4-6", "claude-opus-4-6", "claude-haiku-4-5"]:
            captures = []
            home = root / f"home-{mode}-{model}"
            home.mkdir(exist_ok=True)
            env = {
                k: os.environ[k]
                for k in ["PATH", "LD_LIBRARY_PATH", "NIX_LD", "NIX_LD_LIBRARY_PATH"]
                if k in os.environ
            }
            env.update(
                HOME=str(home),
                CLAUDE_CONFIG_DIR=str(home),
                ANTHROPIC_BASE_URL=f"http://127.0.0.1:{server.server_port}",
                CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1",
                DISABLE_AUTOUPDATER="1",
                TMPDIR=str(root),
            )
            env["ANTHROPIC_API_KEY" if mode == "key" else "CLAUDE_CODE_OAUTH_TOKEN"] = (
                "sk-ant-test-local-only"
            )
            args = [
                shutil.which("claude"),
                "-p",
                "Reply exactly OK.",
                "--model",
                model,
                "--tools",
                "Read",
                "--strict-mcp-config",
                "--mcp-config",
                '{"mcpServers":{}}',
                "--setting-sources",
                "",
                "--no-session-persistence",
            ]
            try:
                result = subprocess.run(
                    args, cwd=home, env=env, capture_output=True, text=True, timeout=35
                )
                print(
                    mode,
                    model,
                    result.returncode,
                    result.stdout[:200],
                    result.stderr[:200],
                    flush=True,
                )
            except subprocess.TimeoutExpired:
                print(mode, model, "timeout", flush=True)
            (root / f"{mode}-{model}.json").write_text(json.dumps(captures, indent=2))
finally:
    server.shutdown()
    server.server_close()
