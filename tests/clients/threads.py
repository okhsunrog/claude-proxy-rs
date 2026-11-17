"""Exercise thread continuations through the real beta SDK in both modes."""
import os

import anthropic

client = anthropic.Anthropic(
    base_url=os.environ["PROXY_AUDIT_URL"], api_key=os.environ["PROXY_AUDIT_KEY"],
    max_retries=0, timeout=15,
    default_headers={"x-proxy-client-profile": "openai"},
)
base = dict(model="claude-sonnet-4-6", max_tokens=100,
            betas=["message-threads-2026-08-12"],
            tool_choice={"type": "tool", "name": "read_file"})
tools = [{"name": "read_file", "input_schema": {"type": "object", "properties": {
    "path": {"type": "string"}}, "required": ["path"], "additionalProperties": False}}]
for streaming in (False, True):
    create = dict(base, tools=tools, thread={"type": "create"},
                  messages=[{"role": "user", "content": "audit thread"}])
    if streaming:
        with client.beta.messages.stream(**create) as stream:
            first = stream.get_final_message()
    else:
        first = client.beta.messages.create(**create)
    call = first.content[0]
    assert call.name == "read_file" and call.input == {"path": "probe.txt"}
    request = dict(base, thread={"type": "continue", "previous_message_id": first.id}, messages=[{
        "role": "user", "content": [{"type": "tool_result", "tool_use_id": call.id,
        "content": [{"type": "tool_reference", "tool_name": "read_file"}]}]}])
    if streaming:
        with client.beta.messages.stream(**request) as stream:
            second = stream.get_final_message()
    else:
        second = client.beta.messages.create(**request)
    assert second.content[0].name == "read_file"
    assert second.content[0].input == {"path": "probe.txt"}
    try:
        client.beta.messages.create(**dict(request, system=None))
    except anthropic.BadRequestError:
        pass
    else:
        raise AssertionError("Continue must reject even a null system field")
    request["thread"]["previous_message_id"] = "missing-thread"
    try:
        client.beta.messages.create(**request)
    except anthropic.NotFoundError as error:
        assert error.body["error"]["details"]["error_code"] == "thread_not_found"
    else:
        raise AssertionError("Missing thread must trigger replay")
client.close()
print("SDK thread create/continue passed: JSON, SSE, missing state, forbidden fields")
