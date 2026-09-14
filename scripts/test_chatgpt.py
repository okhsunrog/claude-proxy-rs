"""Live GPT smoke tests: uv run --env-file .env --with openai scripts/test_chatgpt.py --model MODEL"""
import argparse
import json
import os
from openai import OpenAI, BadRequestError

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--model', required=True)
args = parser.parse_args()
client = OpenAI(base_url=os.environ['PROXY_BASE_URL'].rstrip('/') + '/v1', api_key=os.environ['PROXY_API_KEY'], timeout=120, max_retries=0)
model = args.model
messages = [{'role': 'user', 'content': 'Reply exactly OK.'}]
chat = client.chat.completions.create(model=model, messages=messages)
assert chat.choices[0].message.content and chat.usage.total_tokens > 0
print('Chat JSON:', chat.choices[0].message.content)
text = ''
usage = None
finished = False
for chunk in client.chat.completions.create(model=model, messages=messages, stream=True, stream_options={'include_usage': True}):
    if chunk.choices:
        text += chunk.choices[0].delta.content or ''
        finished |= chunk.choices[0].finish_reason is not None
    if chunk.usage:
        usage = chunk.usage
assert text and finished and usage and usage.total_tokens > 0
print('Chat SSE:', text)
response = client.responses.create(model=model, input='Reply exactly OK.')
assert response.output_text and response.usage.total_tokens > 0
print('Responses JSON:', response.output_text)
text = ''
completed = None
for event in client.responses.create(model=model, input='Reply exactly OK.', stream=True):
    if event.type == 'response.output_text.delta':
        text += event.delta
    elif event.type == 'response.completed':
        completed = event.response
assert text and completed and completed.output and completed.usage.total_tokens > 0
print('Responses SSE:', text)
tool = {'type': 'function', 'function': {'name': 'get_value', 'description': 'Get the test value', 'parameters': {'type': 'object', 'properties': {}, 'additionalProperties': False}, 'strict': True}}
request_messages = [{'role': 'user', 'content': 'Use get_value and report the returned value.'}]
response = client.chat.completions.create(model=model, messages=request_messages, tools=[tool], tool_choice={'type': 'function', 'function': {'name': 'get_value'}})
assert response.choices[0].finish_reason == 'tool_calls'
message = response.choices[0].message
calls = message.tool_calls
assert calls and calls[0].function.name == 'get_value'
json.loads(calls[0].function.arguments)
result = client.chat.completions.create(model=model, messages=request_messages + [message.model_dump(exclude_none=True)] + [{'role': 'tool', 'tool_call_id': call.id, 'content': '42'} for call in calls], tools=[tool], tool_choice='none')
assert '42' in (result.choices[0].message.content or '')
print('Chat function round trip:', result.choices[0].message.content)
arguments = ''
name = ''
finish = None
for chunk in client.chat.completions.create(model=model, messages=request_messages, tools=[tool], tool_choice={'type': 'function', 'function': {'name': 'get_value'}}, stream=True):
    if chunk.choices:
        choice = chunk.choices[0]
        finish = choice.finish_reason or finish
        for call in choice.delta.tool_calls or []:
            name += call.function.name or ''
            arguments += call.function.arguments or ''
assert name == 'get_value' and finish == 'tool_calls'
json.loads(arguments)
print('Chat streamed function arguments: OK')
native_tool = {'type': 'function', **tool['function']}
response = client.responses.create(model=model, input=request_messages, tools=[native_tool], tool_choice={'type': 'function', 'name': 'get_value'})
calls = [item for item in response.output if item.type == 'function_call']
assert calls
history = request_messages + [item.model_dump(exclude_none=True) for item in response.output] + [{'type': 'function_call_output', 'call_id': call.call_id, 'output': '42'} for call in calls]
result = client.responses.create(model=model, input=history, tools=[native_tool], tool_choice='none')
assert '42' in result.output_text
print('Responses function and reasoning replay:', result.output_text)
try:
    client.responses.create(model=model, input='Hello', previous_response_id='unsupported')
except BadRequestError:
    print('Unsupported stored continuation rejected: OK')
else:
    raise AssertionError('Expected rejection of previous_response_id')
print('All GPT smoke tests passed')
