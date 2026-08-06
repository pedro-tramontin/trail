<h1 align="center">Trail</h1>

<p align="center">
  Passive workday capture, daily summary, VPS push.<br>
  A Tauri 2 menu-bar app for macOS.
</p>

<p align="center">
  <a href="https://github.com/pedro-tramontin/trail/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/pedro-tramontin/trail/release.yml?branch=main&label=release&logo=github" alt="Release build"></a>
  <a href="https://github.com/pedro-tramontin/trail/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/pedro-tramontin/trail/ci.yml?branch=main&label=ci&logo=github" alt="CI"></a>
  <a href="https://github.com/pedro-tramontin/trail/releases"><img src="https://img.shields.io/github/v/release/pedro-tramontin/trail?include_prereleases&sort=semver&logo=github" alt="Release"></a>
  <a href="https://github.com/pedro-tramontin/trail/blob/main/LICENSE"><img src="https://img.shields.io/github/license/pedro-tramontin/trail?logo=github" alt="License"></a>
  <a href="https://github.com/pedro-tramontin/trail/releases"><img src="https://img.shields.io/github/downloads/pedro-tramontin/trail/total?logo=github" alt="Downloads"></a>
  <a href="https://github.com/pedro-tramontin/trail/commits/main"><img src="https://img.shields.io/github/last-commit/pedro-tramontin/trail/main?logo=github" alt="Last commit"></a>
</p>

Trail is a Tauri menu-bar app that quietly captures what you did today — GitHub PRs,
Claude sessions, calendar events, voice notes — and writes a daily summary you approve
before it pushes to your VPS. All summarization is local (`ollama`); only the approved
JSON crosses the network.

![Trail menu-bar popover](docs/screenshots/menu-bar.png)

## Features

- **GitHub collector** — captures PRs opened/merged/closed today, fetches review thread comments.
- **Claude sessions collector** — reads `~/.claude/projects/<workspace>/*.jsonl`, summarizes per-session outcomes.
- **Calendar collector** — pulls today's events from a local `.ics` file (subscribed ICS URL works too).
- **Voice capture** — push-to-talk hotkey, transcribes locally with whisper.cpp (`base.en` model).
- **Local summarizer** — `ollama` (default `gpt-oss:20b`) with optional cloud catalog; cloud API keys live in macOS Keychain, never in config.
- **Anonymization** — optional generic-category pass (`[AUTH-INFRA]`, `[BACKEND-SVC]`, etc.) for when the summary travels to shared docs.
- **SSH transport** — pushes approved JSON to your VPS via a keypair generated and stored in macOS Keychain on first run.
- **Demo mode** — first-run flag (`--demo`) shows the UI with fixture data so you can poke around without setting up everything.
- **LLM-driven onboarding** — interactive setup that walks you through the first collector, the first keychain entry, and the first push.
- **Logs UI + capture history** — every raw capture is viewable; failed-validation files are surfaced, not silently dropped.
- **Transport trait** — typed `Transport` (`#[non_exhaustive]`) so v2 can add `HttpsTransport`, `LocalTransport`, `S3Transport`, `DatabaseTransport` as one-day adds.

![Trail Review window](docs/screenshots/review-window.png)

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
2. **Summarize.** At `review_time` (default 18:00), `ollama` reads the raw captures and
   produces a draft `DaySummary` JSON conforming to `day-summary.schema.json`.
3. **Review.** The Review window opens, shows the draft, lets you edit + annotate.
4. **Push.** When you click "Push to VPS", the (optionally anonymized) JSON is sent
   via SSH to your VPS. The collector appends it to that day's plan file.

## Quick start

### Download

Grab the prebuilt release from the [Releases page](https://github.com/pedro-tramontin/trail/releases):

- **macOS** — `Trail-<version>-universal.dmg` (Intel + Apple Silicon)

Trail needs:

- macOS 12 or newer
- Apple Silicon or Intel (the DMG is universal)
- `ollama` installed and running (for summarization — optional in demo mode)

### `trail-collector` (VPS binary)

If you're running Trail's VPS-side collector yourself (not using a managed Trail Cloud
endpoint), install on the VPS:

```bash
cargo install trail-collector --git https://github.com/pedro-tramontin/trail
```

This installs a single static binary to `~/.cargo/bin/trail-collector`. The collector
has zero runtime dependencies — it's a Rust binary built against the host's libc. Runs
on any Linux VPS with glibc or musl.

If the git host is unreachable, you can also install from a local checkout:

```bash
make install-collector   # cargo install --path crates/trail-collector --locked
```

See `crates/trail-collector/` for the configuration schema and a bundled CLI
`--health` self-test.

### Build from source

See [`docs/developer.md`](docs/developer.md) for the full toolchain list. The short
version:

```bash
# Prereqs: Rust ≥ 1.78, Node ≥ 22, pnpm ≥ 11, Tauri 2 system deps for macOS
git clone https://github.com/pedro-tramontin/trail
cd trail
pnpm install
cargo run --bin trail           # launch the desktop app
```

A `Makefile` at the repo root orchestrates the cross-language build:

```bash
make install-collector   # install the VPS-side collector (cargo install --path crates/trail-collector --locked)
make dev                 # pnpm tauri dev — run the Tauri app (requires a display)
make build               # cargo build --workspace
make test                # cargo test --workspace
make lint                # cargo clippy --workspace --all-targets -- -D warnings
make fmt                 # cargo fmt --all -- --check
make fmt-check           # cargo fmt --all -- --check (alias)
make clean               # cargo clean
```

### Development

```bash
# During development
cd src && pnpm dev        # Vite dev server (HMR for the UI)
cargo run --bin trail     # in another terminal — starts the Tauri app pointed at the dev server

# Before opening a PR
make lint && make test
```

The headless build host (CI, the Linux agent) cannot run the Tauri window — visual
verification is a separate step on a real macOS desktop. The honest claim from the
headless agent is "the binary launches, the IPC bridge initializes, the engine
starts." See [`docs/developer.md`](docs/developer.md) for the full inner dev loop.

## Documentation

- **[`docs/architecture.md`](docs/architecture.md)** — laptop + VPS topology, data flow, the transport trait, invariants, where to look when adding a collector or a Tauri command.
- **[`docs/developer.md`](docs/developer.md)** — building, running, testing, debugging, common dev tasks, the code review checklist.
- **[`docs/security.md`](docs/security.md)** — threat-model controls, the macOS Keychain boundary, PEM `Zeroizing<String>` hardening, the supply-chain policy, what to do if CI fails on an advisory.
- **[`docs/e2e-runbook.md`](docs/e2e-runbook.md)** — the end-to-end SSH push + collector `--once` test against a real VPS, including the skip-mode default for headless hosts.
- **[`CONTRIBUTING.md`](CONTRIBUTING.md)** — how to contribute (issues, PRs, AI policy, code review checklist).

## Security

Trail is a single-user tool that handles secrets (SSH private keys, optional cloud LLM
API keys) and ships PII-adjacent content (calendar attendees, voice transcripts, Claude
session outcomes). The threat-model controls and the supply-chain enforcement are
documented in detail at [`docs/security.md`](docs/security.md). The short version:

- **All summarization runs locally**; only the user-approved JSON crosses the network.
- **The SSH private key is generated in-process** and wrapped in `Zeroizing<String>` so
  the heap bytes are zeroed on drop.
- **Voice transcription happens fully on-device** via whisper.cpp; no audio leaves the laptop.
- **Calendar event bodies are never captured** — only start/end times, attendees, and
  organizer (per `~/.trail/collector.json`'s `schema_validation: strict`).
- **Cloud API keys live in macOS Keychain**, never in `~/.trail/config.json`.

If you find a security issue, **do not open a public issue** — see
[`CONTRIBUTING.md`](CONTRIBUTING.md#security).

## License

[Apache-2.0](LICENSE).

