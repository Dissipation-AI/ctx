# ctx: bash in a ctx tab. Read the files a login bash reads, then ctx's prompt and command marks.
[ -r /etc/profile ] && . /etc/profile
if [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
elif [ -r "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
elif [ -r "$HOME/.profile" ]; then . "$HOME/.profile"; fi
[ -n "$CTX_SHIMS" ] && PATH="$CTX_SHIMS:$PATH"
[ -n "$CTX_LAUNCHERS" ] && case ":$PATH:" in *":$CTX_LAUNCHERS:"*) ;; *) PATH="$PATH:$CTX_LAUNCHERS" ;; esac
[ -n "$CTX_PREFILL" ] && history -s "$CTX_PREFILL"
unset CTX_PREFILL CTX_INTEGRATION
