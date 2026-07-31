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
#
# IMPORTANT (§5b D1 fix): the JSON + cron paths are rendered on the
# SERVER using the server's $HOME, NOT the developer's laptop $HOME.
# The heredoc body is single-quoted (<<'REMOTE_EOF') so neither
# client-side nor server-side variable expansion happens inside the
# body; REMOTE_HOME and REMOTE_DIR are explicit server-side locals
# so their values resolve inside the heredoc body via normal bash
# expansion. REMOTE_DIR is forwarded from the laptop via "$1" so
# the server (which doesn't know the arg) has access.

# Step 5 + 6 + 7 are folded into a single SSH roundtrip so the
# server does all the path rendering in one shot. The same heredoc
# body is printed verbatim in --dry-run mode, so the dry-run is an
# honest preview of what the server receives (with $HOME / $1 left
# intact for the operator to verify by eye).
#
# `bash -s -- "$REMOTE_DIR"` runs the heredoc body with $REMOTE_DIR
# bound to $1. NOTE: the outer REMOTE_EOF heredoc delimiter is
# SINGLE-QUOTED so neither client- nor server-side expansion happens
# inside the body; variables inside the body expand via normal bash
# rules at execution time on the SERVER.
REMOTE_BODY="$(cat <<'REMOTE_EOF'
set -euo pipefail
REMOTE_HOME="$HOME"
REMOTE_DIR="$1"
REMOTE_CONFIG="$REMOTE_HOME/.trail/collector.json"
REMOTE_LOG="$REMOTE_HOME/.trail/collector.log"
CRON_MARKER="trail-collector --config"

# Render collector.json on the server side.
cat > "$REMOTE_CONFIG" <<JSON_EOF
{
  "inbox_dir":         "$REMOTE_HOME/.trail/inbox",
  "processed_dir":     "$REMOTE_HOME/.trail/processed",
  "failed_dir":        "$REMOTE_HOME/.trail/failed",
  "plan_root":         "$REMOTE_HOME/.hermes/plans/career-coaching-pedro/daily",
  "plan_template":     "{date}.md",
  "schema_path":       "$REMOTE_HOME/.trail/schema/day-summary.schema.json",
  "log_path":          "$REMOTE_HOME/.trail/collector.log",
  "user":              "${SUDO_USER:-${USER:-}}",
  "schema_validation": "strict"
}
JSON_EOF
echo "wrote $REMOTE_CONFIG"

# Install the cron entry idempotently (strip prior marker line, then
# append). NOTE: cron does not honour shell-style quoting in its
# command field; paths with spaces must be avoided. Our paths
# contain no spaces, so a bare substitution is safe.
CRON_LINE="*/5 * * * * $REMOTE_DIR/trail-collector --config $REMOTE_CONFIG once >> $REMOTE_LOG 2>&1"
( crontab -l 2>/dev/null | grep -v -F "$CRON_MARKER" || true ) \
    | { cat; echo "$CRON_LINE"; } | crontab -
echo "installed cron: $CRON_LINE"

# Create the working dirs the collector --health check requires.
mkdir -p \
    "$REMOTE_HOME/.trail/inbox" \
    "$REMOTE_HOME/.trail/processed" \
    "$REMOTE_HOME/.trail/failed" \
    "$REMOTE_HOME/.hermes/plans/career-coaching-pedro/daily" \
    && echo "mkdirs ok"
REMOTE_EOF
)"

# For dry-run: show the full command the server will receive, including
# the `bash -s -- "$REMOTE_DIR"` invocation — this lets the operator
# sanity-check the path-forwarding by eye. For a real run: hand the
# body to `bash -s -- "$REMOTE_DIR"` over SSH.
if [[ "$DRY_RUN" -eq 1 ]]; then
    printf '  [dry-run] ssh %s bash -s -- %s\n' "$VPS_HOST" "$REMOTE_DIR"
    printf '%s\n' "$REMOTE_BODY" | sed 's/^/    /'
else
    printf '  >> ssh %s bash -s -- %s\n' "$VPS_HOST" "$REMOTE_DIR"
    # The body is passed to the remote shell via stdin (here-string);
    # $REMOTE_DIR is intentionally expanded client-side so it travels
    # as $1 to the remote bash (which then sets it as REMOTE_DIR=$1
    # at the top of the body). Inside the body, $HOME and all the
    # REMOTE_* locals resolve server-side via bash's normal rules.
    # shellcheck disable=SC2029
    ssh "$VPS_HOST" "bash -s -- '$REMOTE_DIR'" <<<"$REMOTE_BODY"
fi

# ---- step 6: post-install health probe --------------------------------
# Runs the collector's --health mode with the freshly-written config.
# A successful run prints {"ok": true, ...} and exits 0.
# Use ~ on the server side (the remote shell expands it); the binary
# path is built from the client-side $REMOTE_DIR flag for display only.

ssh_cmd "post-install: $REMOTE_BIN --config ~/.trail/collector.json health" \
    "$REMOTE_BIN --config ~/.trail/collector.json health"

# ---- done -------------------------------------------------------------

if [[ "$DRY_RUN" -eq 1 ]]; then
    echo
    echo "(dry-run complete; no commands were executed)"
else
    echo
    echo "install complete. cron will run $REMOTE_BIN --once every 5 minutes."
    echo "manual run: ssh $VPS_HOST $REMOTE_BIN --config ~/.trail/collector.json once"
    echo "tail the log: ssh $VPS_HOST tail -f ~/.trail/collector.log"
fi
