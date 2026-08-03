# Security

Trail is a single-user tool that handles secrets (SSH private keys, optional cloud
LLM API keys) and ships PII-adjacent content (calendar attendees, voice transcripts,
Claude session outcomes, GitHub review comments). The threat-model controls and the
supply-chain enforcement are documented here.

For the high-level design (where the boundaries are), see
[`architecture.md`](architecture.md). For how to report a security issue, see
[`CONTRIBUTING.md`](../CONTRIBUTING.md#security).

## Threat model

Trail is a passive capture tool. The assets we protect are:

- **The SSH private key** stored in the macOS Keychain. Used for the daily push.
- **Optional cloud LLM API keys** (only if a cloud catalog is configured in
  `~/.trail/config.json`). Stored in the macOS Keychain.
- **Voice transcripts and PII-adjacent content** (calendar attendees, Claude session
  summaries, GitHub review comments) until the user pushes the approved summary to
  the VPS. The VPS only ever sees the approved JSON; the raw captures stay on the
  laptop.
- **The integrity of the daily summary** (the file on the VPS that's appended to the
  plan file). The JSON schema validation on both sides is the integrity boundary.

The threat actors we design against:

- **A network attacker** between the laptop and the VPS. Mitigated by SSH (host
  verification + the keypair, no password auth).
- **A local attacker** on the laptop with disk access. Mitigated by Keychain (the
  SSH key never touches the filesystem; the config is `~/.trail/config.json` with
  default umask) and by the `Zeroizing<String>` PEM wrapping.
- **A compromised npm package** or **Rust crate** in the supply chain. Mitigated by
  the `pnpm audit` + `cargo deny` policy in the release pipeline, plus SHA-pinned
  GitHub Actions.

We do **not** design against:

- A compromised macOS itself (the Keychain is the root of trust; if the OS is owned,
  all bets are off).
- A compromised VPS root (the collector writes to the configured paths; if the
  VPS is owned, the attacker can modify the plan files regardless).
- A compromised `ollama` server (the laptop is the trust boundary for the
  summarization; a malicious ollama response is rejected by the schema validator).

## Threat-model controls

### Local summarization is the default

The `ollama` client in `src-tauri/src/ollama.rs` is the load-bearing piece. The
default config points at `http://localhost:11434`, which means:

- No raw capture (GitHub PRs, Claude sessions, calendar events, voice transcripts)
  ever leaves the laptop unless the user explicitly approves a push.
- The summarizer's only outbound network call is to localhost. The Tauri
  webview's CSP is locked to bundled assets (`default-src 'self'`, plus explicit
  `base-uri 'none'`, `form-action 'none'`, `object-src 'none'`, `frame-ancestors
  'none'`).
- Cloud LLM catalog endpoints (if configured) only ever receive the **post-
  summarization** draft, never the raw captures. The UI's "Push to VPS" button is
  the only way the user-validated JSON crosses the network.

### SSH keypair stays in the macOS Keychain

The SSH transport (`src-tauri/src/transport.rs`) reads the private key bytes
fresh on every `push()` call. The flow:

1. On first run, if no keypair is present in the Keychain, the app generates an
   `ed25519` keypair via the `ssh-key` crate.
2. The private key bytes are stored in the Keychain under
   `service="trail", account="ssh-key"`.
3. The public key is written to `~/.trail/id_trail.pub` so the user can `ssh-copy-id`
   it to the VPS.
4. On every push, the app reads the private key bytes from the Keychain
   (via `keyring::read_private_key_*()`), wraps them in `Zeroizing<String>`, and
   passes them to `ssh2::Session::userauth_pubkey_memory` for in-memory
   authentication.
5. The `Zeroizing<String>` is dropped at the end of the `push` call. The heap
   bytes are zeroed on drop (CWE-316 / ASVS V6.4.1).

**The private key never touches the filesystem.** Not in `~/.trail/`, not in
`~/.ssh/`, not in temp. The bytes live in the Keychain and in the SSH agent's
authentication handshake — that's it.

### PEM bytes are `Zeroizing<String>`-wrapped

In `src-tauri/src/keyring.rs` (the Keychain reader) and `src-tauri/src/transport.rs`
(the SSH auth), the PEM bytes are wrapped in `zeroize::Zeroizing<String>`. This:

- Sets the heap bytes to zero on drop (so a post-mortem memory dump doesn't leak
  the key).
- Prevents the compiler from optimizing the zeroing away.
- Is `#[derive(Debug)]`-skipped — the key bytes never appear in a `Debug` output,
  log line, or panic message.

The PR review §1.3 flagged a non-blocking informational finding here: the PEM
bytes were briefly held as plain `String` in the early return path. The fixup
squashed the `Zeroizing` wrap into the first push call site. The follow-up PR
extended the wrap to every other code path that handles the bytes.

### Voice transcription is fully on-device

The voice module (`src-tauri/src/voice/`) uses `whisper-rs` to run whisper.cpp
locally with the `base.en` model. The audio bytes are:

- Captured via `cpal` into an in-memory buffer (no on-disk `.wav` is written).
- Resampled to 16 kHz mono via `rubato` (in-memory).
- Fed to `whisper-rs` for transcription. The transcription is the only thing that
  leaves the audio path.

The macOS microphone permission is one-shot — denying requires manual
System Settings fix (Privacy & Security → Microphone → Trail). The app surfaces
the deny in the UI with a "Open System Settings" button.

### Calendar event bodies are never captured

The calendar collector (`crates/trail-collector/src/collectors/calendar.rs`) only
extracts:

- Event start / end time
- Event title
- Attendees
- Organizer

Event **bodies** (the long description, the location notes, the URL attachments)
are never read by the collector. The `schema_validation: strict` setting in
`~/.trail/collector.json` enforces this on the VPS side; the laptop-side collector
mirrors the same field allowlist.

### Cloud API keys live in macOS Keychain

If a cloud LLM catalog is configured in `~/.trail/config.json` (e.g. an OpenAI
endpoint for higher-quality summarization), the API key is read from the macOS
Keychain under `service="trail", account="cloud-llm-<provider>"`. The key is:

- Never written to `~/.trail/config.json` (the config holds the Keychain service
  name, not the key).
- Read fresh on every summarizer call (no in-process caching).
- Cleared from the summarizer's local state after the call.

The Tauri command surface never returns the key to the UI. The UI shows a masked
version (last 4 chars) for confirmation.

## Supply-chain enforcement

Every PR runs the `release.yml` workflow with three supply-chain gates. All three
are blocking.

- **`pnpm audit --audit-level=moderate`** — fails on any JS advisory at moderate
  severity or above.
- **`cargo deny check advisories`** — fails on any Rust advisory (the deny.toml
  ignore list pins known upstream-blocked ones).
- **SHA-pinned GitHub Actions** — every `uses: <action>@<ref>` line in
  `.github/workflows/*.yml` is pinned to a commit SHA, not a tag or branch. The
  release pipeline uses `softprops/action-gh-release@3bb12739c298aeb8a4eeaf626c5b8d85266b0e65`
  (v2.1.1) — see the §7.6 audit log for the rationale.

### Adding a new dependency?

- For npm: add it to `src/package.json`, run `pnpm install`, then commit the
  `pnpm-lock.yaml`. CI will run the audit on your PR.
- For cargo: add it to the relevant `Cargo.toml` (workspace deps if shared), then
  commit the `Cargo.lock`. CI will run `cargo deny check advisories` on your PR.

### If CI fails on an advisory you didn't introduce

- Check if it's a new one (you can fix it by bumping the affected dep).
- If it's an upstream-blocked one (matches an ID in `deny.toml`), follow the
  re-evaluation rules before adding a new `ignore` entry.
- For JS advisories, `pnpm audit` will tell you the recommended bump; most are
  resolved by `pnpm update --latest <package>`.

## Anonymizer scope

The optional anonymizer (`src-tauri/src/anonymizer.rs`) is a best-effort regex
pass. It is **not** a privacy guarantee. The pass:

- Replaces specific project / service names with generic placeholders
  (`[AUTH-INFRA]`, `[BACKEND-SVC]`, `[DATA-PIPELINE]`, `[FRONTEND-UI]`).
- Replaces specific people's names with role placeholders (`[PM]`, `[ENG-LEAD]`).
- Leaves the day's date, the win/blocker/people structure, and the
  count of items unchanged.

**What's NOT anonymized:**

- Free-text quotes from the user (the "summary" field is the LLM output, which
  the anonymizer does not parse).
- Any field the user added by hand in the Review window.
- Voice transcripts (they're a separate field and the user is expected to
  anonymize them by hand if needed).
- Calendar event titles (the calendar collector only captures the title — the
  anonymizer doesn't touch the title field).

If you need stronger anonymization, edit the draft in the Review window by hand
before pushing. The anonymizer is the safety net for "I forgot to redact the
project name in the wins list," not the primary mechanism.

## Reporting a security issue

If you find a security issue, **do not open a public issue**. Email the
maintainer directly (see the GitHub profile). For non-security bugs, open a
public issue.

## Security checklist for new features

If you're adding a new collector, a new transport, or a new persistence path,
walk through this:

- [ ] No new secrets are written to `~/.trail/config.json` (use the Keychain)
- [ ] No new secrets are written to `~/.trail/` at all (Keychain or env vars only)
- [ ] Any new private-key material is `Zeroizing<String>`-wrapped
- [ ] Any new outbound HTTP client uses `rustls-tls` (no `native-tls`)
- [ ] Any new file write is `O_NOFOLLOW` (or equivalent on macOS) for the path
      components
- [ ] Any new Tauri command returns errors as `String` (no debug dumps)
- [ ] Any new user-supplied path is validated against the per-source schema before
      being written to disk
- [ ] The new feature is mentioned in the PR description's "Security" section
