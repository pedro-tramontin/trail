# E2E Verification Log

> Template. Copy this file to a per-run location (e.g. `docs/verification/e2e-<date>-<host>.md` or scratchpad) before running `bash tests/e2e_collector.sh`. Fill in the bracketed placeholders as you go. The 7 step sections correspond 1:1 to the script's `--- N. ... ---` markers.

---

## Run metadata

- **Date / time (UTC):** `[YYYY-MM-DD HH:MM UTC]`
- **Operator:** `[name]`
- **Host:** `[hostname (e.g. pedro-mbp.local)]`
- **Repo:** `pedro-tramontin/trail`
- **Branch:** `[feat/<...> e.g. feat/1-7-e2e-verify]`
- **Commit:** `[git rev-parse HEAD]`
- **TRAIL_E2E_HOST:** `[user@host of the VPS under test — does NOT appear in code, only here]`
- **TRAIL_E2E_BINARY:** `[local path to the collector binary]`
- **TRAIL_E2E_SSH_KEY:** `[path to the SSH private key]`

## Pre-flight result

> Run BEFORE the e2e. Both must be green for the run to be meaningful.

- [ ] `tailscale status` — VPS appears online / reachable
- [ ] `ssh-add -L` — the public key matching `TRAIL_E2E_SSH_KEY` is loaded
- [ ] `TRAIL_E2E_BINARY --help` exits 0 and lists `health`/`once`/`validate`
- [ ] `bash -n tests/e2e_collector.sh` exit 0

## Step results

> Each step shows: timestamp → PASS / FAIL → verbatim stderr or remote output. The script prints `=== E2E FAILED at line N ===` on the line of the first failing step; record that line here.

### Step 1 — install collector + schema + config on VPS

- Timestamp: `[HH:MM:SS UTC]`
- Result: **PASS** / FAIL
- Notes:

```text
[paste any relevant ssh / scp output here, or "ok" if nothing unusual]
```

### Step 2 — `--health` smoke

- Timestamp: `[HH:MM:SS UTC]`
- Exit code: `[0]`
- Result: **PASS** / FAIL
- `--health` stdout (JSON):

```text
[paste the JSON the collector printed, e.g. {\"ok\": true, ...}]
```

### Step 3 — push test day-summary

- Timestamp: `[HH:MM:SS UTC]`
- Result: **PASS** / FAIL
- Notes:

```text
[the inboxed file path + size]
```

### Step 4 — `--once`

- Timestamp: `[HH:MM:SS UTC]`
- Exit code: `[0 or 2]`
- Result: **PASS** / FAIL
- Notes:

```text
[any tracing output from the collector log via `ssh <host> tail ~/.trail-e2e/<tag>/collector.log`]
```

### Step 5 — plan file appended

- Timestamp: `[HH:MM:SS UTC]`
- Result: **PASS** / FAIL
- Plan file path on VPS: `[~/.trail-e2e/<tag>/plans/<YYYY-MM-DD>.md]`
- First 30 lines of the plan file:

```text
[paste the plan file head]
```

### Step 6 — source JSON moved to processed/

- Timestamp: `[HH:MM:SS UTC]`
- Inbox file present (expect NO): `[no]`
- `processed/<date>.json` present (expect YES): `[yes]`
- Result: **PASS** / FAIL

### Step 7 — `--validate` rejects a bad file

- Timestamp: `[HH:MM:SS UTC]`
- Exit code: `[1]`
- `--validate` stdout/stderr:

```text
[paste the JSON payload; expect {\"ok\": false, ...}]
```

- Result: **PASS** / FAIL

## Verdict

- Final result: **PASS** / FAIL
- Failure summary (FAIL only):

```text
[Which step failed, the exact error line, the remote stdout/stderr, and any hypothesis on the root cause. Common causes: SSH key not in agent (PassphraseRequired), VPS-side $HOME differs from $REMOTE_DIR assumption (CWE-665 re-check), schema_path doesn't exist on the VPS, collector.json missing a required key.]
```

## Sign-off

- [ ] All 7 steps PASS
- [ ] Verdict recorded above
- [ ] Any related issues / fixes filed in the project's tracker
