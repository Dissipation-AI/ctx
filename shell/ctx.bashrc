# ctx: bash in a ctx tab. Read the files a login bash reads, then ctx's prompt and command marks.
[ -r /etc/profile ] && . /etc/profile
if [ -r "$HOME/.bash_profile" ]; then . "$HOME/.bash_profile"
elif [ -r "$HOME/.bash_login" ]; then . "$HOME/.bash_login"
elif [ -r "$HOME/.profile" ]; then . "$HOME/.profile"; fi
[ -n "$CTX_SHIMS" ] && PATH="$CTX_SHIMS:$PATH"
[ -n "$CTX_LAUNCHERS" ] && case ":$PATH:" in *":$CTX_LAUNCHERS:"*) ;; *) PATH="$PATH:$CTX_LAUNCHERS" ;; esac
[ -n "$CTX_PREFILL" ] && history -s "$CTX_PREFILL"
# A tab an agent opened: no pager waits for a person to press q, and no history expansion, so a
# command runs as it would in the agent's own shell (`print("done!")` isn't "event not found").
[ -n "$CTX_NO_PAGER" ] && export PAGER=cat GIT_PAGER=cat MANPAGER=cat GH_PAGER=cat PSQL_PAGER=cat AWS_PAGER= SYSTEMD_PAGER= && set +H
unset CTX_PREFILL CTX_INTEGRATION CTX_NO_PAGER
