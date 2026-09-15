"""Live Messages compatibility tests against a registered GPT model."""
import argparse
import os
from anthropic import Anthropic, BadRequestError

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--model', required=True)
args = parser.parse_args()
client = Anthropic(base_url=os.environ['PROXY_BASE_URL'].rstrip('/'), api_key=os.environ['PROXY_API_KEY'], timeout=180, max_retries=0)
base = dict(model=args.model, max_tokens=256)
messages = [{'role': 'user', 'content': 'Reply exactly OK.'}]
raw = client.messages.with_raw_response.create(**base, messages=messages, extra_body={'context_management': {'edits': [{'type': 'clear_thinking_20251015', 'keep': 'all'}]}})
assert 'context_management' in raw.headers['x-proxy-compatibility-warnings']
assert 'max_output_tokens' in raw.headers['x-proxy-compatibility-warnings']
reply = raw.parse()
assert any(b.type == 'text' and b.text for b in reply.content)
assert reply.stop_reason == 'end_turn' and reply.usage.output_tokens > 0
print('Messages JSON and compatibility warning: OK')
with client.messages.stream(**base, messages=messages) as stream:
    text = ''.join(stream.text_stream)
    reply = stream.get_final_message()
assert text and reply.stop_reason == 'end_turn' and reply.usage.output_tokens > 0
print('Messages SSE collected by SDK: OK')
try:
    client.messages.create(**base, messages=messages, extra_headers={'x-proxy-compatibility': 'strict'})
except BadRequestError as e:
    assert 'max_output_tokens' in str(e)
else:
    raise AssertionError('strict mode must reject unsupported max_tokens')
print('Strict control rejection: OK')

tools = [{'name': 'get_value', 'description': 'Get the test value', 'input_schema': {'type': 'object', 'properties': {'label': {'type': 'string'}}, 'required': ['label'], 'additionalProperties': False}}]
history = [{'role': 'user', 'content': 'Use get_value with label test, then report its returned value.'}]
for streaming in [False, True]:
    options = dict(**base, messages=history, tools=tools, tool_choice={'type': 'tool', 'name': 'get_value'}, thinking={'type': 'adaptive'})
    if streaming:
        with client.messages.stream(**options) as stream:
            reply = stream.get_final_message()
    else:
        reply = client.messages.create(**options)
    calls = [b for b in reply.content if b.type == 'tool_use']
    assert calls and reply.stop_reason == 'tool_use'
    assert all(isinstance(c.input, dict) and c.name == 'get_value' for c in calls)
    signatures = [b.signature for b in reply.content if b.type == 'thinking']
    assert all(s.startswith('llm-relay:responses:v1:') for s in signatures)
    continuation = history + [{'role': 'assistant', 'content': [b.model_dump(exclude_none=True) for b in reply.content]}, {'role': 'user', 'content': [{'type': 'tool_result', 'tool_use_id': c.id, 'content': '42'} for c in calls]}]
    result = client.messages.create(**base, messages=continuation, tools=tools, tool_choice={'type': 'none'})
    assert any(b.type == 'text' and '42' in b.text for b in result.content)
    print(f'Messages {"SSE" if streaming else "JSON"} tool round trip: OK')
reasoning_history = [{'role': 'user', 'content': 'Reason through a tricky arithmetic check before answering: is 127*83 greater than 10500, and by how much?'}]
for streaming in [False, True]:
    options = dict(**base, messages=reasoning_history, thinking={'type': 'adaptive'})
    if streaming:
        with client.messages.stream(**options) as stream:
            reply = stream.get_final_message()
    else:
        reply = client.messages.create(**options)
    signatures = [b.signature for b in reply.content if b.type == 'thinking']
    assert signatures and all(s.startswith('llm-relay:responses:v1:') for s in signatures)
    result = client.messages.create(**base, messages=reasoning_history + [{'role': 'assistant', 'content': [b.model_dump(exclude_none=True) for b in reply.content]}, {'role': 'user', 'content': 'What was the difference? Reply with just the number.'}])
    assert any(b.type == 'text' and '41' in b.text for b in result.content)
    print(f'Messages {"SSE" if streaming else "JSON"} signed reasoning replay: OK')

# Optional fields must stay optional when Messages tools become Responses tools.
read_tool = {
    'name': 'Read',
    'description': 'Read a file. Only provide pages when reading a PDF.',
    'input_schema': {
        'type': 'object',
        'properties': {
            'file_path': {'type': 'string'},
            'pages': {'type': 'string', 'description': 'PDF page range, for PDF files only.'},
        },
        'required': ['file_path'],
        'additionalProperties': False,
    },
}
for streaming in [False, True]:
    options = dict(**base, messages=[{'role': 'user', 'content': 'Read /tmp/README.md, a short plain Markdown file.'}], tools=[read_tool], tool_choice={'type': 'tool', 'name': 'Read'})
    if streaming:
        with client.messages.stream(**options) as stream:
            reply = stream.get_final_message()
    else:
        reply = client.messages.create(**options)
    calls = [b for b in reply.content if b.type == 'tool_use']
    assert calls and calls[0].input == {'file_path': '/tmp/README.md'}, calls
    print(f'Messages {"SSE" if streaming else "JSON"} optional tool argument omitted: OK')

print('All Messages GPT smoke tests passed')
