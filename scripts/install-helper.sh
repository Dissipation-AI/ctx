#!/bin/sh
# Install the ctx helper (the root daemon that starts agents' tabs as their own accounts).
# Build it first as yourself (cargo build --release -p ctx-helper), then as an administrator: sudo scripts/install-helper.sh <owner> <agent>=<account>...
#   e.g. sudo scripts/install-helper.sh c dev=vibe-daemon sysadmin=sys-ops-monkey
# Undo: sudo scripts/install-helper.sh --remove
set -eu
BIN=/Library/PrivilegedHelperTools/ctx-helper
PLIST=/Library/LaunchDaemons/com.ctx.helper.plist
RULES="/Library/Application Support/ctx/helper.toml"
[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }
if [ "${1:-}" = --remove ]; then
    launchctl bootout system "$PLIST" 2>/dev/null || true
    rm -f "$BIN" "$PLIST" /var/run/ctx-helper.sock
    echo "Removed. The rules stay in $RULES."
    exit 0
fi
[ $# -ge 2 ] || { sed -n 3,6p "$0" >&2; exit 2; }
owner=$1; shift
id -u "$owner" >/dev/null
here=$(cd "$(dirname "$0")/.." && pwd)
src="$here/target/release/ctx-helper"
[ -x "$src" ] || { echo "build ctx-helper first (as $owner: cargo build --release -p ctx-helper)" >&2; exit 1; }
mkdir -p /Library/PrivilegedHelperTools "$(dirname "$RULES")"
install -o root -g wheel -m 755 "$src" "$BIN"
{
    echo "owner = \"$owner\""
    echo "[accounts]"
    for pair in "$@"; do
        agent=${pair%%=*}; account=${pair#*=}
        id -u "$account" >/dev/null
        echo "$agent = \"$account\""
    done
} > "$RULES.new"
chown root:wheel "$RULES.new"; chmod 644 "$RULES.new"; mv "$RULES.new" "$RULES"
install -o root -g wheel -m 644 "$here/ctx-helper/com.ctx.helper.plist" "$PLIST"
launchctl bootout system "$PLIST" 2>/dev/null || true
launchctl bootstrap system "$PLIST"
echo "Installed. Rules:"; cat "$RULES"
