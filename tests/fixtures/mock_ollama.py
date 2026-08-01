#!/usr/bin/env python3
"""Phase 3 §3.6 mock ollama server. Serves canned 5-section Markdown
on POST /api/generate + returns 200 on GET /api/tags (health check).
Run as: python3 mock_ollama.py <port>
"""
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

CANNED_RESPONSE = """\
## Summary

Worked on the trail-collector refactor and the day-summary schema migration.

## Wins

- Merged the §3.0 prompts PR
- Wrote the summarizer pipeline tests

## Blockers

- None

## People

- Spoke with ACME Corp's team about the integration.

## Open threads

- Need to follow up on the bootstrap LRU compaction.
"""


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/api/tags":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps({"models": [{"name": "llama3"}]}).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == "/api/generate":
            length = int(self.headers.get("Content-Length", 0))
            _ = self.rfile.read(length)  # discard body
            payload = {
                "model": "llama3",
                "response": CANNED_RESPONSE,
                "done": True,
            }
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            self.wfile.write(json.dumps(payload).encode())
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        # Quieter output.
        pass


def main():
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 11434
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"mock_ollama listening on http://127.0.0.1:{port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
