#!/usr/bin/env python3
"""Test Anthropic native API"""

import os
from anthropic import Anthropic

# Get API key from environment variable
api_key = os.getenv("PROXY_API_KEY")
if not api_key:
    raise ValueError("PROXY_API_KEY environment variable is required")

base_url = os.getenv("PROXY_BASE_URL", "http://127.0.0.1:4096")
client = Anthropic(
    base_url=base_url,
    api_key=api_key
)

print("=" * 60)
print("Testing Anthropic Native API")
print("=" * 60)

# Test 1: Basic message
print("\n1. Basic message (non-streaming):")
response = client.messages.create(
    model="claude-sonnet-4-5",
    max_tokens=100,
    messages=[{"role": "user", "content": "Say 'Hello from Anthropic API!' and nothing else."}]
)
print(f"Response: {response.content[0].text}")
print(f"Usage: {response.usage.input_tokens + response.usage.output_tokens} tokens")

# Test 2: Streaming
print("\n2. Streaming message:")
stream = client.messages.create(
    model="claude-sonnet-4-5",
    max_tokens=100,
    messages=[{"role": "user", "content": "Count from 1 to 5, one number per line."}],
    stream=True
)
print("Stream output: ", end="", flush=True)
for event in stream:
    if event.type == "content_block_delta":
        if hasattr(event.delta, 'text'):
            print(event.delta.text, end="", flush=True)
print("\n")

# Test 3: Count tokens
print("\n3. Count tokens:")
token_count = client.messages.count_tokens(
    model="claude-sonnet-4-5",
    messages=[{"role": "user", "content": "Hello, how are you today?"}]
)
print(f"Token count: {token_count.input_tokens} tokens")

print("\n✅ Anthropic API tests completed successfully!")
