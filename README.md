# Trail

> Passive workday capture, daily summary, VPS push.
> Tauri 2 menu-bar app for macOS.

[![Release](https://img.shields.io/github/v/release/pedro-tramontin/trail?style=flat-square)](https://github.com/pedro-tramontin/trail/releases)
[![License](https://img.shields.io/github/license/pedro-tramontin/trail?style=flat-square)](./LICENSE)
[![CI](https://img.shields.io/github/actions/workflow/status/pedro-tramontin/trail/release.yml?style=flat-square&label=release)](https://github.com/pedro-tramontin/trail/actions)
[![Draft build](https://img.shields.io/github/actions/workflow/status/pedro-tramontin/trail/draft-build.yml?style=flat-square&label=draft-build)](https://github.com/pedro-tramontin/trail/actions)

Trail is a Tauri menu-bar app that quietly captures what you did today — GitHub PRs,
Claude sessions, calendar events, voice notes — and writes a daily summary you approve
before it pushes to your VPS. All summarization is local (`ollama`); only the approved
JSON crosses the network.

![Trail menu-bar popover](docs/screenshots/menu-bar.png)

## Features

- **GitHub collector** — captures PRs opened/merged/closed today, fetches review thread comments.
- **Claude sessions collector** — reads `~/.claude/projects/<workspace>/*.jsonl`,
  summarizes per-session outcomes.
- **Calendar collector** — pulls today's events from a local `.ics` file
  (subscribed ICS URL works too).
- **Voice capture** — push-to-talk hotkey, transcribes locally with whisper.cpp
  (`base.en` model).
- **Local summarizer** — `ollama` (default `gpt-oss:20b`) with optional cloud
  catalog; cloud API keys live in macOS Keychain, never in config.
- **Anonymization** — optional generic-category pass (`[AUTH-INFRA]`,
  `[BACKEND-SVC]`, etc.) for when the summary travels to shared docs.
- **SSH transport** — pushes approved JSON to your VPS via a keypair generated
  and stored in macOS Keychain on first run.
- **Demo mode** — first-run flag (`--demo`) shows the UI with fixture data so
  you can poke around without setting up everything.

![Trail Review window](docs/screenshots/review-window.png)

## Install

### macOS app

Download the latest `Trail-<version>-universal.dmg` from
[Releases](https://github.com/pedro-tramontin/trail/releases), double-click,
drag to `/Applications`.

Trail needs:

- macOS 12 or newer
- Apple Silicon or Intel (the DMG is universal)
- `ollama` installed and running (for summarization — optional in demo mode)

### `trail-collector` (VPS binary)

If you're running Trail's VPS-side collector yourself (not using a managed
Trail Cloud endpoint):

```bash
cargo install trail-collector --git https://github.com/pedro-tramontin/trail
```

This installs a single static binary to `~/.cargo/bin/trail-collector`. The
collector has zero runtime dependencies — it's a Rust binary built against the
host's libc. Runs on any Linux VPS with glibc or musl.

If the git host is unreachable, you can also install from a local checkout:

```bash
make install-collector   # cargo install --path crates/trail-collector --locked
```

See `crates/trail-collector/` for the configuration schema and a bundled CLI
`--health` self-test.

## How it works

```
Laptop (macOS)                             VPS
+-----------+                              +--------+
| Trail.app |  --- SSH (keypair) ---->     | trail- |
|  + ollama |                              | collector
+-----------+                              +--------+
   |   ^
   |   +- summary draft -- approve --- push
   |
   +- raw capture (gh, claude, calendar, voice) -- ~/.trail/raw/<date>/
```

1. **Capture.** Hourly + on-demand collectors write raw JSON to
   `~/.trail/raw/<date>/<source>.json`.
2. **Summarize.** At `review_time` (default 18:00), `ollama` reads the raw
   captures and produces a draft `DaySummary` JSON conforming to
   `day-summary.schema.json`.
3. **Review.** The Review window opens, shows the draft, lets you edit +
   annotate.
4. **Push.** When you click "Push to VPS", the (optionally anonymized) JSON is
   sent via SSH to your VPS. The collector appends it to that day's plan file.

## Configuration

Trail reads `~/.trail/config.json` (laptop) and the collector reads
`~/.trail/collector.json` (VPS). Example laptop config:

```json
{
  "github": { "mode": "gh_cli" },
  "calendar_ics": "~/Library/Calendars/work.calendar/Calendar.ics",
  "voice": { "enabled": true, "hotkey": "ctrl+shift+space" },
  "review_time": "18:00",
  "summarizer": {
    "model": "gpt-oss:20b",
    "use_generic_categories": true
  },
  "transport": {
    "type": "ssh",
    "ssh": {
      "host": "vm.example.com",
      "user": "pedro",
      "auth": "public_key",
      "public_key_path": "~/.ssh/id_trail",
      "remote_path": "/home/pedro/trail/inbox/"
    }
  }
}
```

## Development

```bash
# Install Rust + pnpm + Tauri CLI
cargo install tauri-cli --version "^2.0"
pnpm install

# Run the app in dev mode (requires display — agent hosts skip this)
pnpm tauri dev          # alias for: make dev

# Run the workspace tests
cargo test --workspace
pnpm test               # alias for: make test

# Lint everything
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
pnpm lint               # alias for: make lint
```

PRs welcome. See `docs/CONTRIBUTING.md` for the workflow.

## Security & privacy

- All summarization runs locally; only the user-approved JSON crosses the
  network.
- The SSH private key is generated in-process and wrapped in `Zeroizing<String>`
  so the heap bytes are zeroed on drop.
- Voice transcription happens fully on-device via whisper.cpp; no audio
  leaves the laptop.
- Calendar event **bodies** are never captured — only start/end times,
  attendees, and organizer (per `~/.trail/collector.json`'s
  `schema_validation: strict`).
- The collector binary is a single static artifact with zero runtime
  dependencies.

## License

Apache-2.0 — see [LICENSE](./LICENSE).

---

Built by [@pedro-tramontin](https://github.com/pedro-tramontin).
