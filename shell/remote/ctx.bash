# ctx shell integration for bash: prompt and command marks (OSC 133), the folder (OSC 7), where
# commands run, and the command text. bash 4.4+ uses PS0; older bash (macOS's 3.2) a DEBUG trap.
__ctx_osc() { printf '\033]%s\007' "$1"; }
__ctx_first=1
__ctx_running=0
__ctx_prompt() {
    local st=$?
    [ "$__ctx_first" = 1 ] || __ctx_osc "133;D;$st"
    __ctx_first=0
    __ctx_running=0
    __ctx_osc "6973;where;${USER:-$(id -un)}@${HOSTNAME%%.*}"
    local p=${PWD//%/%25}
    __ctx_osc "7;file://${HOSTNAME}${p// /%20}"
    # The marks go back to the ends each time: the terminal starts a new line at a prompt mark, so
    # anything put in front of it (conda's "(base) ") would sit on a line of its own. The patterns
    # are escaped by hand: bash 3.2 doesn't match backslashes in a quoted "$pattern".
    local ps=${PS1//\\\[\\033]133;A\\007\\\]/}
    PS1='\[\033]133;A\007\]'${ps//\\\[\\033]133;B\\007\\\]/}'\[\033]133;B\007\]'
}
__ctx_cmd() {
    local c
    c=$(HISTTIMEFORMAT='' builtin history 1)
    c=${c#*[0-9]  }
    printf '\033]6973;cmd;%s\007\033]133;C\007' "$c"
}
__ctx_debug() {
    # Once per command line, not for the prompt's own work or completion.
    [ -n "$COMP_LINE" ] && return
    [ "$__ctx_running" = 1 ] && return
    case "$BASH_COMMAND" in __ctx_*) return ;; esac
    __ctx_running=1
    __ctx_cmd
}
PROMPT_COMMAND="__ctx_prompt${PROMPT_COMMAND:+;$PROMPT_COMMAND}"
if [ "${BASH_VERSINFO[0]}" -gt 4 ] || { [ "${BASH_VERSINFO[0]}" -eq 4 ] && [ "${BASH_VERSINFO[1]}" -ge 4 ]; }; then
    PS0='$(__ctx_cmd)'
else
    trap '__ctx_debug' DEBUG
fi
