ZDOTDIR="$BRAINZ_USER_ZDOTDIR"
[[ -r "$ZDOTDIR/.zlogin" ]] && source "$ZDOTDIR/.zlogin"
unset BRAINZ_PROFILE_DIR BRAINZ_USER_ZDOTDIR

# Keep the shared shell history and aliases, but use a compact Brainz prompt.
autoload -Uz add-zsh-hook
function _brainz_prompt() {
    PROMPT='%F{yellow}%1~%f %F{yellow}›%f '
    RPROMPT=''
}
add-zsh-hook precmd _brainz_prompt
_brainz_prompt
