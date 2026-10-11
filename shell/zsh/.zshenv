# ctx: this folder is ZDOTDIR only for the first file zsh reads. Put the user's ZDOTDIR back,
# read their .zshenv, and load ctx's shell integration. zsh then reads the user's own .zprofile,
# .zshrc and .zlogin as usual.
if [[ -n "${CTX_USER_ZDOTDIR+X}" ]]; then
    builtin export ZDOTDIR="$CTX_USER_ZDOTDIR"
    builtin unset CTX_USER_ZDOTDIR
else
    builtin unset ZDOTDIR
fi
{
    builtin typeset _ctx_file="${ZDOTDIR-$HOME}/.zshenv"
    [[ ! -r "$_ctx_file" ]] || builtin source -- "$_ctx_file"
} always {
    if [[ -o interactive && -n "$CTX_INTEGRATION" && -r "$CTX_INTEGRATION" ]]; then
        builtin source -- "$CTX_INTEGRATION"
    fi
    builtin unset _ctx_file
}
