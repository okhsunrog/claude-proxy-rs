"""Upload a recording for transcription without logging credentials.

uv run scripts/transcribe_probe.py recording.ogg --direct
PROXY_API_KEY=... uv run scripts/transcribe_probe.py recording.ogg
"""
import argparse
import json
import mimetypes
import os
from pathlib import Path
import time
import urllib.error
import urllib.request
import uuid


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("audio", type=Path)
    parser.add_argument("--direct", action="store_true", help="Use local subscription credentials")
    parser.add_argument("--auth-file", type=Path, default=Path.home() / ".codex/auth.json")
    parser.add_argument("--base-url", default="http://127.0.0.1:4096")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    audio = args.audio.read_bytes()
    if not audio or len(audio) > 25 * 1024 * 1024:
        parser.error("Recording must be non-empty and at most 25 MiB")
    if args.direct:
        tokens = json.loads(args.auth_file.read_text())["tokens"]
        headers = {
            "Authorization": "Bearer " + tokens["access_token"],
            "originator": "Codex Desktop",
            "User-Agent": "Codex Desktop/26.901.41600 (X11; Linux; x64)",
        }
        if tokens.get("account_id"):
            headers["ChatGPT-Account-Id"] = tokens["account_id"]
        url = "https://chatgpt.com/backend-api/transcribe"
    else:
        token = os.environ.get("PROXY_API_KEY")
        if not token:
            parser.error("Set PROXY_API_KEY for the proxy request")
        headers = {"Authorization": "Bearer " + token}
        url = args.base_url.rstrip("/") + "/v1/audio/transcriptions"
    boundary = "----transcription-probe-" + uuid.uuid4().hex
    filename = "recording" + args.audio.suffix.lower()
    mime = mimetypes.guess_type(filename)[0] or "application/octet-stream"
    body = (
        f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{filename}"\r\n'
        f"Content-Type: {mime}\r\n\r\n"
    ).encode() + audio + f"\r\n--{boundary}--\r\n".encode()
    headers["Content-Type"] = "multipart/form-data; boundary=" + boundary
    request = urllib.request.Request(url, data=body, headers=headers)
    start = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            result = json.load(response)
            print(f"HTTP {response.status}; {time.monotonic() - start:.2f} seconds")
    except urllib.error.HTTPError as error:
        raise SystemExit(f"Transcription failed: HTTP {error.code}") from None
    except urllib.error.URLError:
        raise SystemExit("Cannot reach transcription service") from None
    text = result["text"]
    print(text)
    if args.output:
        args.output.write_text(text, encoding="utf-8")


if __name__ == "__main__":
    main()
