"""Real SDKs against the production HTTP handlers and a deterministic upstream.

Invoked by the ignored Rust compatibility audit; no real provider credentials.
"""
import json
import os
from pathlib import Path

import anthropic
import openai


def run():
    root = Path(os.environ["PROXY_AUDIT_WORKSPACE"])
    root.mkdir(parents=True, exist_ok=True)
    target = root / "probe.txt"
    target.write_text("before\n")
    tools = [
        {
            "name": "read_file",
            "description": "Read a UTF-8 file, or return a tool error if absent.",
            "input_schema": {
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"],
                "additionalProperties": False,
            },
        },
        {
            "name": "edit_file",
            "description": "Replace one exact text occurrence in an existing file.",
            "input_schema": {
                "type": "object",
                "properties": {key: {"type": "string"} for key in ("path", "oldString", "newString")},
                "required": ["path", "oldString", "newString"],
                "additionalProperties": False,
            },
        },
    ]
    mode = os.environ["PROXY_AUDIT_MODE"]
    native = mode.startswith("native")
    streaming = mode.endswith("stream")
    headers = {"x-proxy-client-profile": "openai", "x-proxy-cache-policy": "auto-1h"}
    common = dict(api_key=os.environ["PROXY_AUDIT_KEY"], max_retries=0, timeout=20)
    if native:
        client = anthropic.Anthropic(base_url=os.environ["PROXY_AUDIT_URL"], **common)
    else:
        client = openai.OpenAI(base_url=os.environ["PROXY_AUDIT_URL"] + "/v1", **common)
        tools = [{"type": "function", "function": {"name": t["name"], "description": t["description"], "parameters": t["input_schema"]}} for t in tools]
    messages = [{"role": "user", "content": "audit SDK read, missing file, edit, continue"}]
    for turn in range(4):
        kwargs = dict(model="claude-sonnet-4-6", messages=messages, tools=tools, max_tokens=1000, extra_headers=headers)
        if native:
            if streaming:
                with client.messages.stream(**kwargs) as events:
                    response = events.get_final_message()
            else:
                response = client.messages.create(**kwargs)
            blocks = [b.model_dump(exclude_none=True) for b in response.content]
            calls = [(b.id, b.name, b.input) for b in response.content if b.type == "tool_use"]
            messages.append({"role": "assistant", "content": blocks})
        else:
            if streaming:
                calls_by_index = {}
                with client.chat.completions.create(**kwargs, stream=True) as events:
                    for chunk in events:
                        for choice in chunk.choices:
                            for call in choice.delta.tool_calls or []:
                                item = calls_by_index.setdefault(call.index, {"id": "", "name": "", "arguments": ""})
                                item["id"] += call.id or ""
                                item["name"] += call.function.name or ""
                                item["arguments"] += call.function.arguments or ""
                calls = [(c["id"], c["name"], json.loads(c["arguments"])) for c in calls_by_index.values()]
                assistant = {"role": "assistant", "content": "done" if not calls else None}
                if calls:
                    assistant["tool_calls"] = [{"id": i, "type": "function", "function": {"name": n, "arguments": json.dumps(a)}} for i, n, a in calls]
            else:
                response = client.chat.completions.create(**kwargs)
                assistant = response.choices[0].message.model_dump(exclude_none=True)
                calls = [(c.id, c.function.name, json.loads(c.function.arguments)) for c in response.choices[0].message.tool_calls or []]
            messages.append(assistant)
        if not calls:
            assert turn == 3
            break
        assert len(calls) == 1
        call_id, name, args = calls[0]
        try:
            path = root / args["path"]
            assert path.parent == root
            if name == "read_file":
                result = path.read_text()
            elif name == "edit_file":
                path.write_text(path.read_text().replace(args["oldString"], args["newString"], 1))
                result = "edited"
            else:
                raise AssertionError(f"Unexpected tool: {name}")
            error = False
        except FileNotFoundError:
            result, error = "file missing", True
        assert error == (turn == 1)
        if native:
            messages.append({"role": "user", "content": [{"type": "tool_result", "tool_use_id": call_id, "content": result, "is_error": error}]})
        else:
            messages.append({"role": "tool", "tool_call_id": call_id, "content": result})
    assert target.read_text() == "after\n"

    # Close an actual HTTP stream after observed usage but before message_stop.
    cancel = dict(model="claude-sonnet-4-6", max_tokens=100, messages=[{"role": "user", "content": "audit cancel"}])
    if native:
        with client.messages.stream(**cancel) as events:
            for event in events:
                if event.type == "content_block_delta":
                    break
    else:
        with client.chat.completions.create(**cancel, stream=True) as events:
            for chunk in events:
                if chunk.choices and chunk.choices[0].delta.content:
                    break
    client.close()
    print(json.dumps({"client": mode, "file": "after", "tool_error": True, "continuation": True, "cancelled": True}))


if __name__ == "__main__":
    run()
