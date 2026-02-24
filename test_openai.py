#!/usr/bin/env python3
"""Test OpenAI-compatible API"""

import os
from openai import OpenAI

# Get API key from environment variable
api_key = os.getenv("PROXY_API_KEY")
if not api_key:
    raise ValueError("PROXY_API_KEY environment variable is required")

base_url = os.getenv("PROXY_BASE_URL", "http://127.0.0.1:4096")
client = OpenAI(
    base_url=f"{base_url}/v1",
    api_key=api_key
)

print("=" * 60)
print("Testing OpenAI-Compatible API")
print("=" * 60)

# Test 1: Basic completion
print("\n1. Basic completion (non-streaming):")
response = client.chat.completions.create(
    model="claude-sonnet-4-5",
    max_tokens=100,
    messages=[{"role": "user", "content": "Say 'Hello from OpenAI API!' and nothing else."}]
)
print(f"Response: {response.choices[0].message.content}")
print(f"Usage: {response.usage.total_tokens} tokens")

# Test 2: Streaming
print("\n2. Streaming completion:")
stream = client.chat.completions.create(
    model="claude-sonnet-4-5",
    max_tokens=100,
    messages=[{"role": "user", "content": "Count from 1 to 5, one number per line."}],
    stream=True
)
print("Stream output: ", end="", flush=True)
for chunk in stream:
    if chunk.choices[0].delta.content:
        print(chunk.choices[0].delta.content, end="", flush=True)
print("\n")

print("✅ OpenAI API tests completed successfully!")
