#!/bin/sh
# Install ctx for every account on this computer, agents' accounts included:
#   ctx and ctx-launch in /usr/local/bin;
#   the ctx helper, the root daemon that makes agents' accounts and spaces and starts their tabs;
#   the ctx skills where Claude Code and Codex read skills for every account;
#   on a Mac with Homebrew, /opt/homebrew/bin on every account's PATH, so the agent programs you
#   install with it (brew install --cask claude-code antigravity-cli codex cursor-cli;
#   brew install pi-coding-agent opencode) are every agent's too, one copy you keep up to date.
# Build first as yourself: cargo build --release
# Then as an administrator: sudo scripts/install.sh <owner>   (the account the ctx server runs as)
# Undo: sudo scripts/install.sh --remove   (agents' accounts, homes and spaces stay)
# On a Mac the helper needs Full Disk Access to remove agents (macOS refuses even root without it):
#   System Settings > Privacy & Security > Full Disk Access, + , Cmd-Shift-G, the helper's path.
#   The helper is signed with a certificate made once on this Mac, so that grant survives updates.
set -eu
[ "$(id -u)" = 0 ] || { echo "run with sudo" >&2; exit 1; }
here=$(cd "$(dirname "$0")/.." && pwd)
BIN=/usr/local/bin
SOCK=/var/run/ctx-helper.sock
if [ "$(uname)" = Darwin ]; then
    HELPER=/Library/PrivilegedHelperTools/ctx-helper
    SERVICE=/Library/LaunchDaemons/com.ctx.helper.plist
    RULES="/Library/Application Support/ctx/helper.toml"
    SIGNING="/Library/Application Support/ctx/signing"
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
BREW_PATH=/etc/paths.d/ctx-homebrew
SKILLS="using-ctx interactive-programs"

# Sign the helper with this Mac's own certificate (made the first time), so macOS knows it by
# that certificate rather than by the binary's hash, and Full Disk Access survives each update.
sign_helper() {
    kc="$SIGNING/ctx.keychain-db"
    if [ ! -f "$kc" ]; then
        mkdir -p "$SIGNING"; chmod 700 "$SIGNING"
        /usr/bin/openssl rand -hex 24 > "$SIGNING/password"; chmod 600 "$SIGNING/password"
        cat > "$SIGNING/cert.cnf" <<'EOF'
[req]
distinguished_name=dn
x509_extensions=ext
prompt=no
[dn]
CN=ctx helper
[ext]
basicConstraints=critical,CA:false
keyUsage=critical,digitalSignature
extendedKeyUsage=critical,codeSigning
EOF
        /usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 36500 -config "$SIGNING/cert.cnf" \
            -keyout "$SIGNING/key.pem" -out "$SIGNING/cert.pem" 2>/dev/null
        /usr/bin/openssl pkcs12 -export -inkey "$SIGNING/key.pem" -in "$SIGNING/cert.pem" \
            -out "$SIGNING/id.p12" -passout "file:$SIGNING/password"
        security create-keychain -p "$(cat "$SIGNING/password")" "$kc"
        security unlock-keychain -p "$(cat "$SIGNING/password")" "$kc"
        security import "$SIGNING/id.p12" -k "$kc" -P "$(cat "$SIGNING/password")" -T /usr/bin/codesign >/dev/null
        security set-key-partition-list -S apple-tool:,apple: -s -k "$(cat "$SIGNING/password")" "$kc" >/dev/null
        rm -f "$SIGNING/key.pem" "$SIGNING/id.p12" "$SIGNING/cert.cnf"
    fi
    security unlock-keychain -p "$(cat "$SIGNING/password")" "$kc"
    sha=$(/usr/bin/openssl x509 -noout -fingerprint -sha1 -in "$SIGNING/cert.pem" | sed 's/.*=//; s/://g')
    codesign -f -s "$sha" --keychain "$kc" -i com.ctx.helper "$HELPER"
}

stop_helper() {
    if [ "$(uname)" = Darwin ]; then
        launchctl bootout system "$SERVICE" 2>/dev/null || true
    else
        systemctl disable --now com.ctx.helper 2>/dev/null || true
    fi
}

if [ "${1:-}" = --remove ]; then
    stop_helper
    rm -f "$BIN/ctx" "$BIN/ctx-launch" "$HELPER" "$SERVICE" "$SOCK" "$RULES" "$BREW_PATH"
    [ "$(uname)" = Darwin ] && rm -rf "$SIGNING"
    for s in $SKILLS; do rm -rf "$CLAUDE_SKILLS/$s" "$CODEX_SKILLS/$s"; done
    [ "$(uname)" = Darwin ] || systemctl daemon-reload
    echo "Removed ctx. Agents' accounts, homes and spaces stay; remove them first with ctx if you want them gone."
    exit 0
fi
[ $# = 1 ] || { sed -n 2,14p "$0" >&2; exit 2; }
owner=$1
id -u "$owner" >/dev/null
for b in ctx ctx-launch ctx-helper; do
    [ -x "$here/target/release/$b" ] || { echo "build ctx first (as $owner: cargo build --release)" >&2; exit 1; }
    # A build older than its source would install yesterday's ctx. Cargo's dep-info file
    # (target/release/<bin>.d) lists exactly the source files the binary was built from.
    newer=$(sed 's/^[^:]*: //; s/\\ /%SP%/g' "$here/target/release/$b.d" | tr ' ' '\n' | sed 's/%SP%/ /g' |
        while IFS= read -r f; do
            if [ -n "$f" ] && [ "$f" -nt "$here/target/release/$b" ]; then echo "$f"; break; fi
        done)
    [ -z "$newer" ] || { echo "target/release/$b is older than $newer; rebuild first (as $owner: cargo build --release)" >&2; exit 1; }
done

mkdir -p "$BIN" "$(dirname "$HELPER")" "$(dirname "$RULES")"
install -o root -g "$GROUP" -m 755 "$here/target/release/ctx" "$here/target/release/ctx-launch" "$BIN/"
install -o root -g "$GROUP" -m 755 "$here/target/release/ctx-helper" "$HELPER"
[ "$(uname)" = Darwin ] && sign_helper
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

# Login shells (agents' tabs among them) add the lines of /etc/paths.d to PATH (path_helper).
if [ "$(uname)" = Darwin ] && [ -d /opt/homebrew/bin ]; then
    echo /opt/homebrew/bin > "$BREW_PATH"; chmod 644 "$BREW_PATH"
fi

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
if [ "$(uname)" = Darwin ]; then
    echo "To let ctx remove agents, give $HELPER Full Disk Access (once; it lasts through updates):"
    echo "  System Settings > Privacy & Security > Full Disk Access, +, Cmd-Shift-G, $HELPER"
fi
