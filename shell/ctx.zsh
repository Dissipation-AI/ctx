# ctx shell integration for zsh.
#
# Marks where each prompt, command and output starts and ends (OSC 133), reports the current
# folder (OSC 7), and tells the ctx server privately (OSC 6973, never shown) the command text,
# changes to exported variables, where commands run (user@host) and whether someone has typed
# at the prompt. Nothing here forks a process.

[[ -n "$_ctx_loaded" ]] && return
typeset -g _ctx_loaded=1
typeset -gA _ctx_env
typeset -g _ctx_first=1 _ctx_typing=0 _ctx_where=""
zmodload zsh/parameter 2>/dev/null

_ctx_osc() { builtin print -nr -- $'\e]'"$1"$'\a' }

# Exported variables as "S<US>name<US>value<RS>" entries; only changes after the first report.
_ctx_report_env() {
    # Remote environments aren't saved.
    [[ -n "$CTX_REMOTE" ]] && return
    local k v out="" kind="diff"
    local -A now
    for k in ${(k)parameters[(R)*export*]}; do
        [[ ${parameters[$k]} == *array* || ${parameters[$k]} == *association* ]] && continue
        v=${(P)k}
        v=${v//[$'\e\a\x1e\x1f']/}
        now[$k]=$v
    done
    if (( _ctx_first )); then
        kind="full"
        for k v in "${(@kv)now}"; do out+="S"$'\x1f'"$k"$'\x1f'"$v"$'\x1e'; done
    else
        for k v in "${(@kv)now}"; do
            if [[ -z "${_ctx_env[$k]+X}" || "${_ctx_env[$k]}" != "$v" ]]; then
                out+="S"$'\x1f'"$k"$'\x1f'"$v"$'\x1e'
            fi
        done
        for k in "${(@k)_ctx_env}"; do
            [[ -z "${now[$k]+X}" ]] && out+="U"$'\x1f'"$k"$'\x1e'
        done
    fi
    _ctx_env=("${(@kv)now}")
    [[ -n "$out" || "$kind" == full ]] && _ctx_osc "6973;env;$kind;$out"
}

_ctx_mark_prompt() {
    # Run first, so $? is the command's own status; re-applied each time because prompt themes
    # rebuild PS1 and reorder hooks.
    [[ "${precmd_functions[1]}" == _ctx_precmd ]] || precmd_functions=(_ctx_precmd ${precmd_functions:#_ctx_precmd})
    if [[ "$PS1" != *$'\e]133;A'* ]]; then
        PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
    fi
}

_ctx_precmd() {
    local st=$?
    (( _ctx_first )) || _ctx_osc "133;D;$st"
    local where="${USER:-$USERNAME}@${HOST%%.*}"
    if [[ "$where" != "$_ctx_where" ]]; then
        _ctx_where=$where
        _ctx_osc "6973;where;$where"
    fi
    local p=${PWD//\%/%25}
    p=${p// /%20}
    _ctx_osc "7;file://${HOST}${p}"
    _ctx_report_env
    _ctx_mark_prompt
    # Agent programs run as their agent: ctx's shims stay first on PATH (see `ctx help agents`).
    [[ -z "$CTX_SHIMS" || "${path[1]}" == "$CTX_SHIMS" ]] || path=("$CTX_SHIMS" ${path:#${(b)CTX_SHIMS}})
    if (( _ctx_first )); then
        _ctx_first=0
        _ctx_deferred_init
    fi
}

_ctx_preexec() {
    local cmd=${1//[$'\e\a']/}
    _ctx_osc "6973;cmd;$cmd"
    _ctx_osc "133;C"
}

_ctx_line_redraw() {
    local t=$(( ${#BUFFER} > 0 ))
    if (( t != _ctx_typing )); then
        _ctx_typing=$t
        _ctx_osc "6973;typing;$t"
    fi
}

_ctx_line_finish() {
    if (( _ctx_typing )); then
        _ctx_typing=0
        _ctx_osc "6973;typing;0"
    fi
}

# ssh: an interactive session (no remote command) gets ctx's marks on the remote side through
# an inline bootstrap, so remote commands are recorded like local ones. Everything else (a remote
# command, scp/sftp/rsync/git, hosts skipped in ctx's config or with a RemoteCommand) runs as typed.
ssh() {
    [[ -n "$CTX_REMOTE" || -z "$CTX_SSH_COMMAND" ]] && { command ssh "$@"; return; }
    local a target="" skip_next=0 has_command=0 plain=0
    for a in "$@"; do
        if (( skip_next )); then skip_next=0; continue; fi
        if [[ -z "$target" ]]; then
            case "$a" in
                -[BbcDEeFIiJLlmOoPpQRSWw]) skip_next=1 ;;
                -[NfTGVW]*|-O*) plain=1 ;;
                -*) ;;
                *) target=$a ;;
            esac
        else
            has_command=1
            break
        fi
    done
    if (( plain || has_command )) || [[ -z "$target" || ! -t 0 ]]; then
        command ssh "$@"
        return
    fi
    local host=${target#*@} p skip=0
    for p in ${=CTX_SSH_SKIP}; do [[ "$host" == ${~p} || "$target" == ${~p} ]] && skip=1; done
    if (( ! skip )) && command ssh -G "$@" 2>/dev/null | command grep -qi '^remotecommand '; then
        skip=1
    fi
    _ctx_osc "6973;ssh;start;$target"
    local st
    if (( skip )); then
        command ssh "$@"
    else
        command ssh -t "$@" "$CTX_SSH_COMMAND"
    fi
    st=$?
    if (( st == 255 )); then
        _ctx_osc "6973;ssh;end;$target;connection lost"
    else
        _ctx_osc "6973;ssh;end;$target;closed"
    fi
    return $st
}

# Runs at the first prompt, after the user's .zshrc.
_ctx_deferred_init() {
    # Launchers made with `ctx make launcher` run by name (login files may have reset PATH).
    [[ -n "$CTX_LAUNCHERS" && ":$PATH:" != *":$CTX_LAUNCHERS:"* ]] && export PATH="$PATH:$CTX_LAUNCHERS"
    # The line editor may not be loaded yet (macOS's /etc/zshrc loads it with bindkey; other
    # systems don't), and add-zle-hook-widget quietly does nothing without it.
    zmodload zsh/zle 2>/dev/null
    autoload -Uz add-zle-hook-widget 2>/dev/null && {
        add-zle-hook-widget line-pre-redraw _ctx_line_redraw
        add-zle-hook-widget line-finish _ctx_line_finish
    }
    # A reopened tab: its own commands come first on up-arrow.
    if [[ -n "$CTX_REOPEN_HISTORY" && -r "$CTX_REOPEN_HISTORY" ]]; then
        builtin fc -R "$CTX_REOPEN_HISTORY"
    fi
    builtin unset CTX_REOPEN_HISTORY
    # A tab an agent opened: no pager waits for a person to press q, and no history expansion, so a
    # command runs as it would in the agent's own shell (after .zshrc, which may set either).
    if [[ -n "$CTX_NO_PAGER" ]]; then
        builtin export PAGER=cat GIT_PAGER=cat MANPAGER=cat GH_PAGER=cat PSQL_PAGER=cat AWS_PAGER= SYSTEMD_PAGER=
        builtin setopt no_bang_hist
    fi
    builtin unset CTX_NO_PAGER
    # A command that was running when the tab ended: pre-filled, not run.
    if [[ -n "$CTX_PREFILL" ]]; then
        builtin print -z -- "$CTX_PREFILL"
    fi
    # Completion for ctx sentences (tab names complete in quotes), when completion is on.
    if [[ -n "$CTX_COMPLETION" && -r "$CTX_COMPLETION" ]] && (( $+functions[compdef] )); then
        builtin source "$CTX_COMPLETION"
    fi
    builtin unset CTX_PREFILL CTX_INTEGRATION CTX_COMPLETION
    _ctx_osc "6973;hello;1"
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _ctx_precmd
add-zsh-hook preexec _ctx_preexec
