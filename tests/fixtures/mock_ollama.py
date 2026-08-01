#!/usr/bin/env python3
"""Phase 3 §3.6 mock ollama server. Serves canned 5-section Markdown
on POST /api/generate + returns 200 on GET /api/tags (health check).
Run as: python3 mock_ollama.py <port>
"""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

CANNED_RESPONSE_TEMPLATE = """\
## Summary

Worked on the trail-collector refactor and the day-summary schema migration.

## Wins

- Merged the §3.0 prompts PR
- Wrote the summarizer pipeline tests

## Blockers

None

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
        # Read the full body so the e2e harness can verify what the
        # summarizer actually sent. The pre-fix handler discarded the
        # body, so the harness couldn't tell whether the learner
        # bootstrap was injected into the prompt on the second run.
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length).decode("utf-8", errors="replace")
        # Write the body to a file the harness can grep. The file
        # path is `$MOCK_LOG_DIR/$MOCK_LOG_FILE`; both are set by
        # the harness so the mock has no hard-coded knowledge of
        # where to log.
        log_dir = os.environ.get("MOCK_LOG_DIR", "/tmp")
        log_file = os.environ.get("MOCK_LOG_FILE", "mock_ollama-bodies.log")
        try:
            os.makedirs(log_dir, exist_ok=True)
            with open(os.path.join(log_dir, log_file), "a") as f:
                f.write(f"--- POST {self.path} ({length} bytes) ---\n")
                f.write(body)
                f.write("\n")
        except OSError:
            pass  # logging is best-effort

        if self.path == "/api/generate":
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.end_headers()
            # `body` is the JSON request; parse it to extract the
            # user_prompt so the canned response can vary by content
            # (e.g. echo a "bootstrap" marker when the prompt contains
            # the bootstrap block string).
            try:
                req = json.loads(body) if body else {}
            except json.JSONDecodeError:
                req = {}
            prompt = req.get("prompt", "")
            response_text = CANNED_RESPONSE_TEMPLATE
            # Mark a second-run request that includes the bootstrap.
            if "User preferences learned" in prompt:
                response_text = (
                    CANNED_RESPONSE_TEMPLATE
                    + "\n\n<!-- e2e marker: bootstrap-injected -->\n"
                )
            self.wfile.write(
                json.dumps({"response": response_text, "done": True}).encode()
            )
        else:
            self.send_response(404)
            self.end_headers()

    def log_message(self, format, *args):
        # Quieter output.
        pass


def main():
    # Default to 11435 to avoid colliding with a real ollama running
    # on 11434. The e2e harness (tests/e2e_summarizer.sh) sets
    # `MOCK_PORT=11434` when it needs to match the production
    # endpoint; the script-level default is 11435.
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 11435
    server = HTTPServer(("127.0.0.1", port), Handler)
    print(f"mock_ollama listening on http://127.0.0.1:{port}", flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
