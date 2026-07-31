# Trail E2E Test Runbook

The end-to-end test for the Trail collector architecture. Lives at
`tests/e2e_collector.sh`. Runs the full pipeline against a real VPS:
install collector + schema + config → `--health` → push a JSON →
`--once` → assert the plan file is appended + the source JSON moved
to processed/ → `--validate` rejects a malformed file.

## Why this exists

Every other test in the repo is a Rust `#[cfg(test)]` unit test using
`tempfile`-backed fakes. None of them cross a network boundary. The e2e
is the single piece of evidence that the architecture (laptop config →
collector install → SSH push → cron-driven `--once` → plan-file append
on the VPS) works as a vertical slice. If the e2e fails, the failure
is in the architecture, not in the test — the output above
`=== E2E FAILED at line N ===` shows the exact point of breakdown.

## Skip mode

The script defaults to **skip mode** when `TRAIL_E2E_HOST` is unset OR
when `--skip-ssh` is passed. In skip mode it prints a `SKIPPED` line
and exits 0. This makes the script a valid PR-able artifact on hosts
that can't reach the Tailscale-only VPS (e.g. CI, the Linux build
host). The operator re-runs it on the macOS laptop for the real proof.

## Required environment variables

These are read by the script — none are baked into source.

| Variable             | Default                          | Purpose                                                                 |
|----------------------|----------------------------------|-------------------------------------------------------------------------|
| `TRAIL_E2E_HOST`     | *(unset → skip mode)*            | SSH target in `user@host` form. The `user` part becomes the VPS `$HOME` prefix (`/home/<user>/.trail-e2e/<tag>/`) — the script refuses anything that isn't `user@host`. |
| `TRAIL_E2E_BINARY`   | `target/release/trail-collector` | Local path to the collector binary. Relative paths resolve against the repo root. |
| `TRAIL_E2E_SSH_KEY`  | `~/.ssh/id_ed25519`              | SSH private key path. |

## Pre-flight checklist (laptop)

Run each line before invoking the script. Confirm the SSH key is loaded
(Tailscale VPN is up; the VPS hostname resolves).

```bash
# 1. VPN up + VPS visible
tailscale status                              # VPS line shows "online"

# 2. SSH key in the keychain (macOS persists across reboots)
ssh-add --apple-use-keychain ~/.ssh/id_ed25519
ssh-add -L | head -1                          # prints "ssh-ed25519 AAAA… user@host"

# 3. SSH reachability to the VPS
ssh -o BatchMode=yes <user>@<host> 'echo ok'  # expect "ok"

# 4. Local collector built
cargo build --release -p trail-collector
./target/release/trail-collector --help       # lists `health`/`once`/`validate`

# 5. Script syntax-clean (defensive)
bash -n tests/e2e_collector.sh
```

Any failure here is a precondition problem — fix it before running the
e2e; the e2e will surface the same failure but with much less context.

## Running the e2e

```bash
cd ~/code/trail

# Full run against the VPS
TRAIL_E2E_HOST=<user>@<host> bash tests/e2e_collector.sh
```

Expected output shape (clean run):

```
--- preflight: checking reachability of <user>@<host> ---
--- 1. install collector + schema + config on VPS ---
  bin:     /home/<user>/.trail-e2e/trail-e2e-<pid>/bin/trail-collector
  schema:  /home/<user>/.trail-e2e/trail-e2e-<pid>/schema/day-summary.schema.json
  config:  /home/<user>/.trail-e2e/trail-e2e-<pid>/collector.json
--- 2. run --health (assert exit 0 + JSON ok:true) ---
  --health stdout:
    {"ok": true, ...}
--- 3. push a test day-summary JSON to the VPS inbox ---
  inboxed: /home/<user>/.trail-e2e/trail-e2e-<pid>/inbox/<YYYY-MM-DD>.json
--- 4. run --once (assert exit 0) ---
--- 5. verify the plan file was appended ---
  plan file contents:
    <rendered section>
--- 6. verify the source JSON moved to processed/ ---
  file is no longer in inbox; processed/<YYYY-MM-DD>.json present
--- 7. run --validate on a BAD file (assert exit 1 + ok:false) ---
  --validate exit code: 1
  --validate stdout/stderr:
    {"ok": false, ...}
=== E2E PASSED ===
  test tag:    trail-e2e-<pid>
  date:        <YYYY-MM-DD>
  vps user:    <user>
  vps host:    <host>
  test base:   /home/<user>/.trail-e2e/trail-e2e-<pid>/
  artifacts:
    health output:    /tmp/trail-e2e-<rand>/health.json
    plan output:      /tmp/trail-e2e-<rand>/plan.md
    validate output:  /tmp/trail-e2e-<rand>/validate.json
```

The script exits 0 on PASS and non-zero on FAIL. The trap-based cleanup
removes `/home/<user>/.trail-e2e/<tag>/` on EXIT regardless of outcome,
so the VPS stays clean even if you `Ctrl-C` mid-run.

### Skip-mode runs

```bash
# Explicit skip (no env vars needed)
bash tests/e2e_collector.sh --skip-ssh

# Implicit skip (TRAIL_E2E_HOST unset)
bash tests/e2e_collector.sh
```

Both print `SKIPPED: … — re-run on the macOS laptop.` and exit 0.

## Reading the verification log

A per-run template lives at `templates/e2e-verification-log.md`. Copy
it to a per-run location (e.g. `docs/verification/e2e-<date>.md` or
just a scratchpad), then fill in the bracketed placeholders as the
script prints output:

| Script output line             | Log section                                           |
|--------------------------------|-------------------------------------------------------|
| `--- N. ... ---`               | The matching `### Step N` heading                     |
| `=== E2E FAILED at line N ===` | Bottom of the file (`## Verdict` → `Failure summary`) |
| `=== E2E PASSED ===`           | Bottom (`## Verdict` → `Final result: PASS`)          |
| `--health stdout: <json>`      | `### Step 2 — --health` → `--health stdout (JSON)`    |
| `plan file contents:`          | `### Step 5 — plan file appended` → `First 30 lines`  |
| `--validate stdout/stderr:`    | `### Step 7 — --validate rejects a bad file`          |

The verification log is durable evidence: if a later phase needs to
audit whether the architecture was ever green end-to-end, this log +
the git tag on the matching commit is the answer.

## Failure debugging

Start with the line number in `=== E2E FAILED at line N ===`. The
script's `set -Eeuo pipefail` aborts on the first error; the line
number points at the exact assertion that failed.

Common causes by step:

| Step | Symptom | Likely cause |
|------|---------|--------------|
| 1 (install) | `scp` permission denied | SSH key not loaded (`ssh-add -L` is empty); or wrong `TRAIL_E2E_SSH_KEY` |
| 1 (install) | `mkdir` refused | VPS user's home is read-only (rare); check `ssh <host> "touch ~/.canary"` |
| 2 (health) | No `"ok": true` in output | `schema_path` or `inbox_dir` doesn't exist on the VPS; verify with `ssh <host> "ls -la ~/.trail-e2e/<tag>/schema"` |
| 4 (once) | Exit code 2 | `tracing` log on the VPS shows the validation error: `ssh <host> "cat ~/.trail-e2e/<tag>/collector.log"` |
| 5 (plan) | Section missing | Step 4 silently failed; check the collector log first |
| 7 (validate) | Exit 0 on a bad file | Schema isn't `strict`, or `--validate` has a regression — file an issue with the log attached |

If the failure isn't recoverable in 10 minutes, paste the full script
output back into the conversation — the line numbers + the verbatim
remote stderr are usually enough for the agent to narrow it down.

## What this test is NOT

- Not in `cargo test` — it's a bash integration smoke against a real
  VPS, not a unit test. `cargo test --workspace` is still the gate for
  Rust coverage.
- Not run on every push — it's slow, depends on network + the SSH key,
  and produces artifacts in `/tmp` + on the VPS. Run it manually when
  the architecture changes (a new collector mode, a new transport, a
  new schema field) or before tagging a release.
- Not a load test — it processes one file. Cron-driven `--once` runs
  every 5 minutes in production; if the inbox is ever saturated (10+
  files), check `~/.trail/collector.log` on the VPS for backpressure.
