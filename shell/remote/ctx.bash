# ctx shell integration for bash (remote side): prompt and command marks (OSC 133), the folder
# (OSC 7), where commands run, and the command text (bash 4.4+, through PS0).
__ctx_osc() { printf '\033]%s\007' "$1"; }
__ctx_first=1
__ctx_prompt() {
    local st=$?
    [ "$__ctx_first" = 1 ] || __ctx_osc "133;D;$st"
    __ctx_first=0
    __ctx_osc "6973;where;${USER:-$(id -un)}@${HOSTNAME%%.*}"
    local p=${PWD//%/%25}
    __ctx_osc "7;file://${HOSTNAME}${p// /%20}"
    case "$PS1" in *'133;A'*) ;; *) PS1='\[\033]133;A\007\]'"$PS1"'\[\033]133;B\007\]' ;; esac
}
__ctx_cmd() {
    local c
    c=$(HISTTIMEFORMAT='' builtin history 1)
    c=${c#*[0-9]  }
    printf '\033]6973;cmd;%s\007\033]133;C\007' "$c"
}
PROMPT_COMMAND="__ctx_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
PS0='$(__ctx_cmd)'
