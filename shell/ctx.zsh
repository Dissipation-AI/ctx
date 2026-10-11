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
        for k v in ${(kv)now}; do out+="S"$'\x1f'"$k"$'\x1f'"$v"$'\x1e'; done
    else
        for k v in ${(kv)now}; do
            if [[ -z "${_ctx_env[$k]+X}" || "${_ctx_env[$k]}" != "$v" ]]; then
                out+="S"$'\x1f'"$k"$'\x1f'"$v"$'\x1e'
            fi
        done
        for k in ${(k)_ctx_env}; do
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
    local where="${USER}@${HOST%%.*}"
    if [[ "$where" != "$_ctx_where" ]]; then
        _ctx_where=$where
        _ctx_osc "6973;where;$where"
    fi
    local p=${PWD//\%/%25}
    p=${p// /%20}
    _ctx_osc "7;file://${HOST}${p}"
    _ctx_report_env
    _ctx_mark_prompt
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

# Runs at the first prompt, after the user's .zshrc.
_ctx_deferred_init() {
    autoload -Uz add-zle-hook-widget 2>/dev/null && {
        add-zle-hook-widget line-pre-redraw _ctx_line_redraw
        add-zle-hook-widget line-finish _ctx_line_finish
    }
    # A reopened tab: its own commands come first on up-arrow.
    if [[ -n "$CTX_REOPEN_HISTORY" && -r "$CTX_REOPEN_HISTORY" ]]; then
        builtin fc -R "$CTX_REOPEN_HISTORY"
    fi
    builtin unset CTX_REOPEN_HISTORY
    # A command that was running when the tab ended: pre-filled, not run.
    if [[ -n "$CTX_PREFILL" ]]; then
        builtin print -z -- "$CTX_PREFILL"
    fi
    builtin unset CTX_PREFILL CTX_INTEGRATION
    _ctx_osc "6973;hello;1"
}

autoload -Uz add-zsh-hook
add-zsh-hook precmd _ctx_precmd
add-zsh-hook preexec _ctx_preexec
