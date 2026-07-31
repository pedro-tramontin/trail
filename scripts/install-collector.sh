#!/usr/bin/env bash
# install-collector.sh — idempotent VPS-side install of the trail-collector
# binary + the master's frozen ~/.trail/collector.json + a `*/5 * * * *`
# cron entry.
#
# Runs from the developer's laptop (macOS, where the musl cross-compile
# target lives) against the VPS over SSH. The VPS doesn't need any
# build tooling — just a user with sudo-less write access to `~/.trail/`
# and to their own crontab.
#
# Usage:
#   ./install-collector.sh \
#     --binary target/x86_64-unknown-linux-musl/release/trail-collector \
#     --host pedro@<vps-host> \
#     --schema resources/day-summary.schema.json \
#     --remote-dir /opt/trail-collector
#
# All flags are required except --schema (default: workspace-root
# resources/day-summary.schema.json) and --remote-dir (default:
# /opt/trail-collector).
#
# --dry-run prints every command it would run but does NOT actually
# SSH / scp. Safe to run on any host.
#
# IDEMPOTENT: re-running does not duplicate the cron entry or the
# binary. The cron install uses `crontab -l | grep -v -F <marker> | crontab -`
# so the prior entry (if any) is stripped before the new one is appended.
# The binary copy uses `scp` which overwrites. The schema copy is the
# same.

set -euo pipefail

# ---- arg parsing --------------------------------------------------------

BINARY=""
VPS_HOST=""
SCHEMA_PATH=""
REMOTE_DIR="/opt/trail-collector"
DRY_RUN=0

usage() {
    cat <<EOF
Usage: $0 --binary <path> --host <user@host> [options]

Options:
  --binary <path>      Local path to the trail-collector binary
                       (typically target/x86_64-unknown-linux-musl/release/trail-collector).
  --host <user@host>   SSH target for the VPS.
  --schema <path>      Local path to the day-summary schema
                       (default: resources/day-summary.schema.json
                       relative to the workspace root).
  --remote-dir <path>  Remote install dir for the binary
                       (default: /opt/trail-collector).
  --dry-run            Print commands without executing them.
  -h, --help           Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary)   BINARY="${2:-}"; shift 2 ;;
        --host)     VPS_HOST="${2:-}"; shift 2 ;;
        --schema)   SCHEMA_PATH="${2:-}"; shift 2 ;;
        --remote-dir) REMOTE_DIR="${2:-}"; shift 2 ;;
        --dry-run)  DRY_RUN=1; shift ;;
        -h|--help)  usage; exit 0 ;;
        *) echo "unknown flag: $1" >&2; usage >&2; exit 2 ;;
    esac
done

if [[ -z "$BINARY" ]]; then
    echo "error: --binary is required" >&2
    usage >&2
    exit 2
fi
if [[ -z "$VPS_HOST" ]]; then
    echo "error: --host is required" >&2
    usage >&2
    exit 2
fi

# Derive defaults that depend on the script's location. The script
# lives at <workspace>/scripts/install-collector.sh, so the workspace
# root is one dirname up.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [[ -z "$SCHEMA_PATH" ]]; then
    SCHEMA_PATH="$WORKSPACE_ROOT/resources/day-summary.schema.json"
fi

# Convert to absolute paths so scp can find them regardless of cwd.
# In dry-run mode we accept the path as-is (no cd required) because
# the caller may pass a placeholder that doesn't exist on disk.
if [[ "$DRY_RUN" -eq 0 ]]; then
    BINARY="$(cd "$(dirname "$BINARY")" && pwd)/$(basename "$BINARY")"
fi
SCHEMA_PATH="$(cd "$(dirname "$SCHEMA_PATH")" && pwd)/$(basename "$SCHEMA_PATH")"

# Skip the file-existence checks in dry-run mode — the smoke test
# passes a placeholder binary that doesn't exist on disk, and we
# don't want dry-run to be a less-honest preview than a real run.
if [[ "$DRY_RUN" -eq 0 ]]; then
    if [[ ! -f "$BINARY" ]]; then
        echo "error: binary not found: $BINARY" >&2
        exit 1
    fi
fi
if [[ ! -f "$SCHEMA_PATH" ]]; then
    echo "error: schema not found: $SCHEMA_PATH" >&2
    exit 1
fi

# Extract VPS user from user@host (the part before @). The user's
# ~/.trail/ lives at /home/<user>/.trail/ on most Linux distros.
VPS_USER="${VPS_HOST%@*}"
if [[ -z "$VPS_USER" || "$VPS_USER" == "$VPS_HOST" ]]; then
    echo "error: --host must be in the form user@host, got: $VPS_HOST" >&2
    exit 2
fi

# ---- runner: dry-run aware exec --------------------------------------

# Print + optionally run a command. The quoting on the captured command
# is purely for the display line — the actual execution uses the
# unquoted array form so words stay split.
run() {
    local display="$1"
    shift
    if [[ "$DRY_RUN" -eq 1 ]]; then
        printf '  [dry-run] %s\n' "$display"
    else
        printf '  >> %s\n' "$display"
        "$@"
    fi
}

# Same as run() but for ssh-with-bare-command (no -tt, no quoting
# in the dry-run display).
ssh_cmd() {
    local display="$1"
    local remote_cmd="$2"
    if [[ "$DRY_RUN" -eq 1 ]]; then
        printf '  [dry-run] ssh %s %s\n' "$VPS_HOST" "$display"
    else
        printf '  >> ssh %s %s\n' "$VPS_HOST" "$display"
        # shellcheck disable=SC2029 # We intentionally expand client-side.
        ssh "$VPS_HOST" "$remote_cmd"
    fi
}

# ---- pre-flight banner -----------------------------------------------

cat <<EOF
== install-collector.sh ==
  binary:        $BINARY
  schema:        $SCHEMA_PATH
  vps host:      $VPS_HOST
  remote dir:    $REMOTE_DIR
  dry-run:       $DRY_RUN
EOF

# ---- step 1: scp the binary ------------------------------------------

REMOTE_BIN="$REMOTE_DIR/trail-collector"
run "scp $BINARY $VPS_HOST:$REMOTE_BIN" \
    scp "$BINARY" "$VPS_HOST:$REMOTE_BIN"

# ---- step 2: chmod +x + ensure remote dir ----------------------------

ssh_cmd "chmod +x $REMOTE_BIN && test -d $REMOTE_DIR && echo ok" \
    "chmod +x '$REMOTE_BIN' && test -d '$REMOTE_DIR' && echo ok"

# ---- step 3: create ~/.trail + the schema dir -------------------------

ssh_cmd "mkdir -p ~/.trail/schema && test -d ~/.trail/schema && echo ok" \
    "mkdir -p ~/.trail/schema && test -d ~/.trail/schema && echo ok"

# ---- step 4: scp the schema file -------------------------------------

run "scp $SCHEMA_PATH $VPS_HOST:~/.trail/schema/day-summary.schema.json" \
    scp "$SCHEMA_PATH" "$VPS_HOST:~/.trail/schema/day-summary.schema.json"

# ---- step 5: write ~/.trail/collector.json ----------------------------
# The 9 fields are the master's frozen schema (mirrors the
# CollectorConfig struct in crates/trail-collector/src/config.rs).
# Paths are derived from the user's home dir so the install is
# portable across VPS flavors (no /home/<user> hardcoding — we use
# ~/ for everything except the binary's --remote-dir).

REMOTE_SCHEMA="$HOME/.trail/schema/day-summary.schema.json"
REMOTE_INBOX="$HOME/.trail/inbox"
REMOTE_PROCESSED="$HOME/.trail/processed"
REMOTE_FAILED="$HOME/.trail/failed"
REMOTE_PLANS="$HOME/.hermes/plans/career-coaching-pedro/daily"
REMOTE_LOG="$HOME/.trail/collector.log"
REMOTE_CONFIG="$HOME/.trail/collector.json"
REMOTE_TEMPLATE="{date}.md"

# Use a heredoc on the remote side so the json is rendered server-side
# from server-known $HOME. We send the JSON as a single-quoted heredoc
# to avoid client-side expansion.
ssh_cmd "write ~/.trail/collector.json" "$(cat <<REMOTE_EOF
cat > $REMOTE_CONFIG <<'JSON_EOF'
{
  "inbox_dir":         "$REMOTE_INBOX",
  "processed_dir":     "$REMOTE_PROCESSED",
  "failed_dir":        "$REMOTE_FAILED",
  "plan_root":         "$REMOTE_PLANS",
  "plan_template":     "$REMOTE_TEMPLATE",
  "schema_path":       "$REMOTE_SCHEMA",
  "log_path":          "$REMOTE_LOG",
  "user":              "$VPS_USER",
  "schema_validation": "strict"
}
JSON_EOF
echo wrote $REMOTE_CONFIG
REMOTE_EOF
)"

# ---- step 6: install the cron entry (idempotent) ----------------------
# The marker is the full cron line (unique per VPS user). Strip any
# prior line that contains "trail-collector --config" then append the
# new line. Using `crontab -` (read from stdin) is portable across
# Linux/macOS crontabs.

CRON_LINE="*/5 * * * * $REMOTE_BIN --config $REMOTE_CONFIG once >> $REMOTE_LOG 2>&1"
CRON_MARKER="trail-collector --config"

# Escape any single quotes in the rendered cron line (defensive — the
# line as built has none, but if someone customizes the marker this
# guard keeps the heredoc sane).
CRON_LINE_ESCAPED="${CRON_LINE//\'/\'\\\'\'}"

ssh_cmd "install cron entry (idempotent)" "$(cat <<REMOTE_EOF
( crontab -l 2>/dev/null | grep -v -F '$CRON_MARKER' || true ) | { cat; echo '$CRON_LINE_ESCAPED'; } | crontab -
echo "installed cron: $CRON_LINE_ESCAPED"
crontab -l | grep -F '$CRON_MARKER' || true
REMOTE_EOF
)"

# ---- step 7: mkdir the working dirs -----------------------------------
# The collector's --health check requires inbox_dir, processed_dir,
# failed_dir, and plan_root to exist. Create them now so the
# post-install health probe is honest.

ssh_cmd "mkdir -p $REMOTE_INBOX $REMOTE_PROCESSED $REMOTE_FAILED $REMOTE_PLANS" \
    "mkdir -p '$REMOTE_INBOX' '$REMOTE_PROCESSED' '$REMOTE_FAILED' '$REMOTE_PLANS' && echo ok"

# ---- step 8: post-install health probe --------------------------------
# Runs the collector's --health mode with the freshly-written config.
# A successful run prints {"ok": true, ...} and exits 0.

ssh_cmd "post-install: $REMOTE_BIN --config $REMOTE_CONFIG health" \
    "$REMOTE_BIN --config $REMOTE_CONFIG health"

# ---- done -------------------------------------------------------------

if [[ "$DRY_RUN" -eq 1 ]]; then
    echo
    echo "(dry-run complete; no commands were executed)"
else
    echo
    echo "install complete. cron will run $REMOTE_BIN --once every 5 minutes."
    echo "manual run: ssh $VPS_HOST $REMOTE_BIN --config $REMOTE_CONFIG once"
    echo "tail the log: ssh $VPS_HOST tail -f $REMOTE_LOG"
fi
