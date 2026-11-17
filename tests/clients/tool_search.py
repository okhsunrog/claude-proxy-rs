"""Real SDK round trip of discovered tools and their search history."""
import os
import anthropic

client = anthropic.Anthropic(base_url=os.environ["PROXY_AUDIT_URL"],
                            api_key=os.environ["PROXY_AUDIT_KEY"], max_retries=0, timeout=15)
tools = [{"type": "tool_search_tool_regex_20251119", "name": "tool_search"}, {
    "name": "directory?", "description": "Read a directory", "defer_loading": True,
    "input_schema": {"type": "object", "properties": {"directory": {"type": "string"}},
                     "required": ["directory"], "additionalProperties": False}}]
messages = [{"role": "user", "content": "audit tool-search"}]
kwargs = dict(model="claude-sonnet-4-6", max_tokens=100, tools=tools)
response = client.beta.messages.create(**kwargs, messages=messages)
content = response.model_dump(exclude_none=True)["content"]
assert content[1]["content"]["tool_references"][0]["tool_name"] == "directory?"
assert content[2]["name"] == "directory?" and content[2]["input"] == {"directory": "folder"}
messages += [{"role": "assistant", "content": content}, {"role": "user", "content": [{
    "type": "tool_result", "tool_use_id": "discovered", "content": "folder read"}]}]
response = client.beta.messages.create(**kwargs, messages=messages)
assert response.content[0].text == "search done"
try:
    client.beta.messages.create(**dict(kwargs, model="claude-sonnet-4-20250514"), messages=messages)
except anthropic.BadRequestError:
    pass
else:
    raise AssertionError("Unsupported model must reject search without stripping it")
client.close()
print("SDK tool search passed: discovery, restored references, follow-up, legacy model rejection")
