# ctx remote bootstrap: sent inline as ssh's remote command (never installed). For a bash or zsh
# login shell it loads the user's normal startup files plus ctx's prompt and command marks, then
# hands over to that shell. Its temporary folder is deleted as soon as the shell has read it.
d=$(mktemp -d 2>/dev/null) || { d="/tmp/ctx-$$"; mkdir -m 700 "$d" || d=""; }
s=${SHELL:-/bin/sh}
n=${s##*/}
if [ -n "$d" ] && [ "$n" = zsh ]; then
cat > "$d/.zshenv" <<'CTXEOF'
builtin unset ZDOTDIR
builtin export CTX_REMOTE=1
[[ -r "$HOME/.zshenv" ]] && builtin source "$HOME/.zshenv"
@ZSH@
command rm -rf -- "$CTX_TMP" 2>/dev/null
builtin unset CTX_TMP
CTXEOF
CTX_TMP=$d ZDOTDIR=$d exec "$s" -l
elif [ -n "$d" ] && [ "$n" = bash ]; then
cat > "$d/rc" <<'CTXEOF'
command rm -rf -- "$CTX_TMP" 2>/dev/null
unset CTX_TMP
export CTX_REMOTE=1
[ -r /etc/profile ] && . /etc/profile
if [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
elif [ -r "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
elif [ -r "$HOME/.profile" ]; then . "$HOME/.profile"; fi
@BASH@
CTXEOF
CTX_TMP=$d exec "$s" --rcfile "$d/rc" -i
else
[ -n "$d" ] && rm -rf -- "$d"
exec "$s" -l
fi
