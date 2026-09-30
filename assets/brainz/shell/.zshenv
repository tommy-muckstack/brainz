# Read the user's normal startup files without changing their configuration.
typeset -g BRAINZ_PROFILE_DIR="$ZDOTDIR"
ZDOTDIR="${BRAINZ_USER_ZDOTDIR:-$HOME}"
[[ -r "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
typeset -g BRAINZ_USER_ZDOTDIR="$ZDOTDIR"
ZDOTDIR="$BRAINZ_PROFILE_DIR"
