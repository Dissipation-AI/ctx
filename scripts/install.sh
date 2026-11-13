#!/bin/sh
# Install ctx for every account on this computer, agents' accounts included:
#   ctx and ctx-launch in /usr/local/bin;
#   the ctx helper, the root daemon that makes agents' accounts and spaces and starts their tabs;
#   the ctx skills where Claude Code and Codex read skills for every account.
# Build first as yourself: cargo build --release
# Then as an administrator: sudo scripts/install.sh <owner>   (the account the ctx server runs as)
# Undo: sudo scripts/install.sh --remove   (agents' accounts, homes and spaces stay)
set -eu
[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }
here=$(cd "$(dirname "$0")/.." && pwd)
BIN=/usr/local/bin
SOCK=/var/run/ctx-helper.sock
if [ "$(uname)" = Darwin ]; then
    HELPER=/Library/PrivilegedHelperTools/ctx-helper
    SERVICE=/Library/LaunchDaemons/com.ctx.helper.plist
    RULES="/Library/Application Support/ctx/helper.toml"
    CLAUDE_SKILLS="/Library/Application Support/ClaudeCode/.claude/skills"
    GROUP=wheel
else
    HELPER=/usr/local/libexec/ctx-helper
    SERVICE=/etc/systemd/system/com.ctx.helper.service
    RULES=/etc/ctx/helper.toml
    CLAUDE_SKILLS=/etc/claude-code/.claude/skills
    GROUP=root
fi
CODEX_SKILLS=/etc/codex/skills
SKILLS="using-ctx interactive-programs"

stop_helper() {
    if [ "$(uname)" = Darwin ]; then
        launchctl bootout system "$SERVICE" 2>/dev/null || true
    else
        systemctl disable --now com.ctx.helper 2>/dev/null || true
    fi
}

if [ "${1:-}" = --remove ]; then
    stop_helper
    rm -f "$BIN/ctx" "$BIN/ctx-launch" "$HELPER" "$SERVICE" "$SOCK" "$RULES"
    for s in $SKILLS; do rm -rf "$CLAUDE_SKILLS/$s" "$CODEX_SKILLS/$s"; done
    [ "$(uname)" = Darwin ] || systemctl daemon-reload
    echo "Removed ctx. Agents' accounts, homes and spaces stay; remove them first with ctx if you want them gone."
    exit 0
fi
[ $# = 1 ] || { sed -n 2,8p "$0" >&2; exit 2; }
owner=$1
id -u "$owner" >/dev/null
for b in ctx ctx-launch ctx-helper; do
    [ -x "$here/target/release/$b" ] || { echo "build ctx first (as $owner: cargo build --release)" >&2; exit 1; }
    # A build older than the source would install yesterday's ctx.
    newer=$(find "$here" -path "$here/target" -prune -o \( -name '*.rs' -o -name Cargo.toml -o -name Cargo.lock \) -newer "$here/target/release/$b" -print | head -1)
    [ -z "$newer" ] || { echo "target/release/$b is older than $newer; rebuild first (as $owner: cargo build --release)" >&2; exit 1; }
done

mkdir -p "$BIN" "$(dirname "$HELPER")" "$(dirname "$RULES")"
install -o root -g "$GROUP" -m 755 "$here/target/release/ctx" "$here/target/release/ctx-launch" "$BIN/"
install -o root -g "$GROUP" -m 755 "$here/target/release/ctx-helper" "$HELPER"
printf 'owner = "%s"\n' "$owner" > "$RULES.new"
chown root:"$GROUP" "$RULES.new"; chmod 644 "$RULES.new"; mv "$RULES.new" "$RULES"

for dir in "$CLAUDE_SKILLS" "$CODEX_SKILLS"; do
    mkdir -p "$dir"
    for s in $SKILLS; do
        rm -rf "${dir:?}/$s"
        cp -R "$here/integrations/ctx-skills/skills/$s" "$dir/$s"
        chown -R root:"$GROUP" "$dir/$s"; chmod -R a+rX,go-w "$dir/$s"
    done
done

stop_helper
if [ "$(uname)" = Darwin ]; then
    install -o root -g wheel -m 644 "$here/ctx-helper/com.ctx.helper.plist" "$SERVICE"
    launchctl bootstrap system "$SERVICE"
else
    cat > "$SERVICE" <<EOF
[Unit]
Description=ctx helper (starts agents' tabs as their own accounts)

[Service]
ExecStart=$HELPER
Restart=always

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now com.ctx.helper
fi
echo "Installed ctx in $BIN, the helper (for $owner), and the skills in $CLAUDE_SKILLS and $CODEX_SKILLS."
