# Developer guide

This is the hands-on guide for working on Trail — building, running, testing, and
debugging. For the high-level design, see [`architecture.md`](architecture.md). For
how to contribute, see [`CONTRIBUTING.md`](../CONTRIBUTING.md). For the threat-model
controls, see [`security.md`](security.md).

## Prerequisites

The full per-OS list is below. The short version:

- **Rust ≥ 1.78** (`rustup update stable`); the `rust-toolchain.toml` pins `stable`
- **Node ≥ 22** and **pnpm ≥ 11**
- **macOS 12+** for the Tauri app (Keychain, menu-bar `tray-icon`, push-to-talk
  microphone permission)
- **Tauri 2 system dependencies** for macOS — Xcode Command Line Tools (`xcode-select
  --install`)
- **`ollama`** running locally with the `gpt-oss:20b` model pulled (or any model
  named in `~/.trail/config.json`)

### macOS

```bash
# Xcode Command Line Tools
xcode-select --install

# Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup update stable

# Node + pnpm
brew install node@22
corepack enable
corepack prepare pnpm@latest --activate

# Ollama (for the local summarizer)
brew install ollama
ollama serve &
ollama pull gpt-oss:20b
```

The Tauri 2 system dependencies on macOS are the Xcode Command Line Tools — no
extra `apt`-style packages are needed. The `tray-icon` feature is enabled in
`src-tauri/Cargo.toml`.

### Linux (build host only)

The Tauri app does not run on Linux in v1 (the master plan's "macOS only for v1"
decision — menu bar, Keychain, push-to-talk microphone are all macOS-specific). But
the workspace and the collector build on Linux:

```bash
# Rust + the system deps for the collector (musl target optional)
sudo apt install build-essential pkg-config libssl-dev

# For a static musl build (what the collector ships as)
rustup target add x86_64-unknown-linux-musl
sudo apt install musl-tools
```

## Building

The project is a Rust workspace with a Tauri 2 desktop app + a standalone VPS-side
collector. The UI (Svelte 5) lives in `src/` and is built by Vite from the repo
root (`pnpm install` installs it). The `pnpm dev` script runs the Vite dev server
out of the repo root; the Tauri app then points at `http://localhost:1420` for HMR.

```bash
# One-time setup
git clone https://github.com/pedro-tramontin/trail
cd trail
pnpm install

# Build the desktop app (debug)
cargo run --bin trail

# Build the desktop app (release, matching the release.yml)
cargo build --release --bin trail
# The .app bundle is at target/release/bundle/macos/Trail.app

# Build the standalone collector (VPS)
cargo build --release -p trail-collector
# The binary is at target/release/trail-collector
```

The `Makefile` is the single source of truth for build orchestration. The
`release.yml` workflow runs the same `make` targets, so what works locally will
work in CI.

## Running a built `.app` on your own Mac (Gatekeeper / unsigned)

Both the local `cargo run` path and the `.dmg` you download from
`Releases` produce `.app` bundles signed **ad-hoc** (Tauri's
`signingIdentity: "-"` in `src-tauri/tauri.conf.json` + a defensive
`codesign --force --deep --sign` step in `promote.yml` / `release.yml`).
Ad-hoc signing satisfies Gatekeeper enough for the bundle to **launch on
the same machine**, but a downloaded bundle still trips "this was
downloaded from the internet" on first launch. Strip the quarantine
attribute before opening:

```bash
# Debug bundle built locally
xattr -dr com.apple.quarantine src-tauri/target/debug/bundle/macos/Trail.app

# Release .dmg artifact from the GitHub Release
xattr -dr com.apple.quarantine /Applications/Trail.app

# Then open
open src-tauri/target/debug/bundle/macos/Trail.app
```

Older macOS (Sonoma and earlier) reports this as **"Trail.app is damaged
and can't be opened"**. The fix is the same — `xattr -cr <path>` clears
all extended attributes at once. If you'd rather click through GUI:
right-click → Open → confirm the "Open anyway" dialog once, and macOS
remembers your approval for that bundle.

Why this isn't a re-signing problem: CI's `codesign --force --deep
--sign "Pedro Tramontin"` step refreshes the ad-hoc signature every
build, so a freshly-extracted bundle is already signed. The only thing
the quarantine flag does is tell Gatekeeper "ask the user first." A
notarized + Developer-ID-signed build (the long-term fix on the
release-pipeline roadmap) would skip this prompt entirely.

## Inner dev loop

For UI work, use Vite's HMR. For Rust work, the cycle is `cargo check` → `cargo test`
→ `cargo run`.

```bash
# Terminal 1: UI dev server (HMR for Svelte 5)
pnpm dev                # runs `vite` from the repo root

# Terminal 2: Tauri app pointed at the dev server
cargo run --bin trail
```

Vite's HMR re-renders Svelte components on save without losing state. Svelte stores
survive the HMR cycle because `src/lib/state/` uses the Svelte 5 `$state` rune (the
runtime preserves runes across HMR updates).

For Rust, the cycle is `cargo check` (fast, no codegen) → `cargo test -p <crate>`
(targeted) → `cargo run --bin trail` (full app). The Makefile has aliases:

```bash
make install-collector   # cargo install --path crates/trail-collector --locked
make dev                 # pnpm tauri dev — run the Tauri app (requires a display)
make build               # cargo build --workspace
make test                # cargo test --workspace
make lint                # cargo clippy --workspace --all-targets -- -D warnings
make fmt                 # cargo fmt --all -- --check
make fmt-check           # cargo fmt --all -- --check (alias)
make clean               # cargo clean
```

## Running tests

```bash
# Rust: full workspace
cargo test --workspace

# Rust: one crate
cargo test -p trail-collector
cargo test -p trail          # the Tauri app crate (lib name `trail_lib`)

# Rust: one test by name
cargo test -p trail transport::tests::name_returns_static_str

# Rust: collector e2e (the bash script against a real VPS — see docs/e2e-runbook.md)
TRAIL_E2E_HOST=<user>@<host> bash tests/e2e_collector.sh

# UI: vitest
pnpm test

# UI: one file
pnpm test src/Onboarding.test.ts
```

Some tests are `#[ignore]`-d by design (live httpbin, H2 multiplexing). To run them:

```bash
cargo test --workspace -- --ignored
```

The ignored tests need network access; they're not part of the default CI run.

## Debugging

### Rust logs

The app uses `tracing` for structured logs. The default log level is `info`; bump
it with `RUST_LOG`:

```bash
RUST_LOG=debug cargo run --bin trail
RUST_LOG=trail_lib=trace,info cargo run --bin trail   # one crate at trace, rest at info
```

For the collector specifically, `RUST_LOG=trail_collector=debug` shows the per-file
validation decisions.

### Tauri DevTools

The Tauri 2 webview exposes DevTools in debug builds. Right-click in the app window
and select "Inspect Element", or use Cmd-Option-I. The DevTools are NOT available
in release builds.

### The collector in isolation

The collector has a CLI that runs without the Tauri shell. Useful for testing the
cron-driven path on your laptop:

```bash
# Build + run the collector against a local config + schema
cargo run -p trail-collector -- --config /path/to/collector.json --health
cargo run -p trail-collector -- --config /path/to/collector.json --once
cargo run -p trail-collector -- --config /path/to/collector.json --validate /path/to/file.json

# Run the e2e SSH test against a real VPS
TRAIL_E2E_HOST=pedro@vm.example.com bash tests/e2e_collector.sh
```

The e2e script defaults to **skip mode** when `TRAIL_E2E_HOST` is unset, so it's
safe to run in any environment — see [`docs/e2e-runbook.md`](e2e-runbook.md) for
the full operator guide.

### The mock SSH server

`tests/fixtures/mock-ssh-server` is a tiny mock that records the SSH pushes the
collector makes. The collector's integration tests use it instead of a real VPS
when the env var `TRAIL_USE_MOCK_SSH=1` is set:

```bash
TRAIL_USE_MOCK_SSH=1 cargo test -p trail-collector
```

The mock captures the exact `scp` payload and writes it to a temp file for
assertion. It's the load-bearing test fixture for the Phase 1 architecture.

## Common dev tasks

### Add a new Tauri command

1. Define the command in the appropriate file under `src-tauri/src/commands.rs` (or
   a new module if the surface is large). The signature is `async fn
   my_command(state: tauri::State<'_, ...>, args: ...) -> Result<T, String>`.
2. Register the command in `src-tauri/src/lib.rs`'s `invoke_handler!` macro.
3. Add a typed wrapper in `src/lib/api.ts`.
4. Add a vitest case for the wrapper (the wrapper is the public contract; the test
   pins the shape).

### Add a new collector

A collector is a source kind on both the laptop and the VPS. They mirror each other:

1. **VPS side**: define the source in
   `crates/trail-collector/src/collectors/<name>.rs` with a `Source::value_variant()`
   arm, a `RawOutput` struct, and a `collect(cfg, date) -> Result<RawOutput,
   CollectorError>` function. Add the schema in
   `crates/trail-collector/schemas/<name>.schema.json`.
2. **Laptop side**: mirror the source kind in `src-tauri/src/collectors.rs`. The
   `dispatch()` factory and the per-source `if let` arms are the load-bearing
   pieces.
3. Add a CLI arm in `crates/trail-collector/src/main.rs` (`Mode::Collect { source }`).
4. Add a config field to `~/.trail/config.json` (laptop) and
   `~/.trail/collector.json` (VPS) — keep the two in sync.
5. Wire the source into the laptop-side scheduler in `src-tauri/src/scheduler.rs`.
6. Add the UI toggle in `src/Settings.svelte` (the per-source enable/disable is in
   the config; the UI just exposes it).
7. Tests: a per-source `tests::collects_<name>` case in the collector module + a
   matching unit test in the laptop-side `collectors.rs` module.

### Add a new transport

1. Implement the `Transport` trait in a new module under
   `src-tauri/src/transport_<name>.rs`. The trait is:
   ```rust
   #[async_trait]
   pub trait Transport: Send + Sync {
       fn name(&self) -> &'static str;
       async fn push(&self, payload: &[u8], remote_name: &str) -> Result<(), TransportError>;
       async fn health_check(&self) -> Result<(), TransportError>;
   }
   ```
2. Add the config variant to `config.rs` (e.g. `Transport::Https { ... }`).
3. Wire the factory in `transport::from_config` (the `match` arm is the
   load-bearing piece).
4. The `#[non_exhaustive]` marker on `Transport`, `TransportError`, and the auth
   enums means new variants are a clean compile, not a refactor.
5. Tests: 4 inline `#[cfg(test)]` cases — `from_config_<name>_dispatch`,
   `name_returns_static_str`, `constructor_preserves_fields`, and one per-auth
   error case (no live network; use `wiremock` or equivalent).

### Add a new domain type

1. Define the type in `src-tauri/src/lib.rs` (or a new module) with
   `#[derive(Debug, Clone, Serialize, Deserialize)]`. The wire format is JSON.
2. If the type is shared with the collector, put it in a place both can import
   (e.g. a new `trail-types` workspace member, or a `pub mod types` in
   `src-tauri/src/lib.rs` that the collector depends on).
3. Add a `serde` `#[serde(rename_all = "snake_case")]` if the on-disk format uses
   snake_case (the existing configs do).
4. Add a unit test for the round-trip: `serde_json::from_str` → re-serialize →
   assert equal.

## Code review checklist

Before opening a PR, walk through this:

- [ ] `cargo fmt --all -- --check` is clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` is clean
- [ ] `pnpm test` passes (count went up, not down)
- [ ] `cargo test --workspace` passes (count went up, not down)
- [ ] The Tauri command is registered in `invoke_handler!` (if you added one)
- [ ] The Tauri command has a typed wrapper in `src/lib/api.ts` (if you added one)
- [ ] The vitest wrapper test exists (if you added a wrapper)
- [ ] New code has tests
- [ ] No `unwrap()` / `expect()` in hot paths (the summarizer loop, the SSH push)
- [ ] No new `#[allow(...)]` without a `// reason:` comment
- [ ] No commented-out code
- [ ] The PR description explains **what** and **why**, not just the diff
- [ ] If the change is a deviation from a documented plan, the PR body calls that
      out

## Performance and profiling

The summarizer loop and the SSH push are the two latency-sensitive paths. If you're
touching `src-tauri/src/summarizer.rs` or `src-tauri/src/transport.rs`:

- Prefer `Bytes` over `String` for byte buffers (use `bytes::Bytes` from the
  workspace deps if you need to add it)
- Prefer `&[u8]` over `Vec<u8>` in function signatures (caller-owned)
- Avoid `format!()` in the per-collector path
- Use `tokio::task::spawn_blocking` for the `ssh2` blocking calls (the existing
  `SshTransport::push` already does this)

For profiling:

```bash
# Build with debug symbols, no LTO
cargo build --release --bin trail --config 'profile.release.lto=false' --config 'profile.release.codegen-units=256'

# Profile with perf (Linux build host)
perf record -F 99 -p $(pgrep trail) -g -- sleep 30
perf report
```

For flamegraphs, the [`cargo-flamegraph`](https://github.com/flamegraph-rs/flamegraph)
tool works out of the box.

## Common gotchas

- **The UI must be installed before `cargo run`.** The `src-tauri` crate's
  `tauri::generate_context!()` proc macro reads `src/dist/index.html` at compile
  time. If you forget `pnpm install` (or the build is stale), the error is a
  `tauri::generate_context!` failure pointing at a missing file.
- **The collector's bundled `trail-collector` binary is the build artifact.** The
  `src-tauri/build.rs` step copies the workspace member's release binary into
  `src-tauri/resources/trail-collector`. If you change the collector, rebuild the
  Tauri app to pick up the new bundle.
- **The Tauri crate is `trail`, not `workday-logger-lib`.** The project was
  renamed from "Workday Logger" to "Trail" in 2026-07-31. Use `cargo test -p trail
  <filter>` for the Tauri app and `cargo test -p trail-collector` for the
  collector.
- **Headless build hosts cannot run the Tauri window.** The `make dev` and
  `cargo run --bin trail` targets require a display. The honest claim from the
  headless agent is "the binary launches, the IPC bridge initializes, the engine
  starts" — visual verification is a separate step on a real macOS desktop.
- **macOS Keychain access is per-binary.** The first push triggers a Keychain
  prompt. If you rebuild the Tauri app from scratch, the new binary is a
  "different app" from Keychain's perspective and the prompt fires again.
