# scribe tmux — the popup behind `scribe tmux`: sessions, entries,
# proposals and backfill candidates in one fzf list, each record previewed
# with `scribe … show`. It only reads: every command it runs is a list or a
# show. The binary runs this text with `sh -c`, and exports:
#   SCRIBE_TMUX_BIN  the scribe executable that was started; every child
#                      command runs it, never whichever scribe is on PATH
#   SCRIBE_TMUX_SH   this text, so fzf's binds and preview can call back in
#
# Keys: enter opens the record (sessions with --log) in $PAGER (else less);
# ctrl-y copies the id (tmux buffer, and pbcopy when present);
# ctrl-s/ctrl-e/ctrl-p/ctrl-b switch to sessions, entries, proposals or
# backfill candidates; alt-a toggles inactive sessions and hidden entries.
set -u

scribe() { "$SCRIBE_TMUX_BIN" "$@"; }
# `show` prints only its list line when stdout is not a terminal (the
# output contract), so the detail views ask for the table format.
detail() { scribe "$@" --format table; }
# `self` is text for fzf's own shell; the popup's direct call uses the function.
self='sh -c "$SCRIBE_TMUX_SH" scribe-tmux'
self_call() { sh -c "$SCRIBE_TMUX_SH" scribe-tmux "$@"; }
state=${SCRIBE_TMUX_STATE:-}

view_file=$state/view
hidden_file=$state/hidden

hidden() { [ -f "$hidden_file" ] && [ "$(cat "$hidden_file")" = 1 ]; }

# The rows come from the binary's hidden `tmux rows` mode: one
# `kind<TAB>id<TAB>display` line per record, built from the same lists the
# list commands report, with a notice row for an empty or failed list. fzf
# shows only the display field. Color is requested when the terminal has one
# (the binary still honors NO_COLOR and TERM=dumb).
list_view() {
	flags=
	if hidden; then flags=--hidden; fi
	case ${TERM:-} in '' | dumb) ;; *) flags="$flags --color" ;; esac
	# shellcheck disable=SC2086
	scribe tmux rows "$1" $flags
}

preview() {
	kind=$1 id=$2
	if [ "$id" = - ]; then
		if [ "$kind" = transcript ]; then
			printf '%s\n' 'No backfill candidate is selected.' \
				'An empty list means no uncataloged transcripts in the window.' \
				'A failed list is this same row; run `scribe transcript list --since <date>` in a shell for the reason.'
		else
			printf '%s\n' 'Nothing is selected: this row is a notice.' \
				'An empty list says so in this row; a failed list shows it too, and running the list command in a shell gives the reason.'
		fi
		return 0
	fi
	case $kind in
	# What the record says first, then its metadata; the pager view keeps
	# the whole record.
	session | entry | proposal) scribe tmux preview "$kind" "$id" ;;
	transcript) detail transcript show "$id" 2>/dev/null || preview transcript - ;;
	*) printf 'unknown record kind: %s\n' "$kind" ;;
	esac
}

copy_id() {
	[ "$1" = - ] && return 0
	if command -v tmux >/dev/null 2>&1; then tmux set-buffer -- "$1" 2>/dev/null || true; fi
	if command -v pbcopy >/dev/null 2>&1; then printf '%s' "$1" | pbcopy 2>/dev/null || true; fi
}

open_record() {
	kind=$1 id=$2
	# A notice row (id "-") opens nothing.
	if [ "$id" = - ] && [ "$kind" != transcript ]; then preview "$kind" -; return 0; fi
	case $kind in
	session) detail session show "$id" --log | ${PAGER:-less} ;;
	entry) detail entry show "$id" | ${PAGER:-less} ;;
	proposal) scribe tmux preview proposal "$id" | ${PAGER:-less} ;;
	transcript)
		if [ "$id" = - ]; then preview transcript -; return 0; fi
		# Daniel catalogs as himself only from his typed prompt: Enter
		# leaves the prompt command in the tmux paste buffer and says so;
		# typed into a Claude Code prompt, the hook records the catalog
		# as his. The CLI refuses --author daniel everywhere.
		cmd="/scribe catalog $id"
		if command -v tmux >/dev/null 2>&1 && tmux set-buffer -- "$cmd" 2>/dev/null; then
			printf 'scribe tmux: paste buffer holds: %s\n' "$cmd"
		else
			printf 'scribe tmux: run from a shell: %s\n' "$cmd"
		fi ;;
	*) preview "$kind" "$id" ;;
	esac
}

# The popup's callbacks: fzf's binds and preview run this text again with
# one of these words first.
case ${1:-} in
_list-current) list_view "$(cat "$view_file" 2>/dev/null || printf sessions)"; exit 0 ;;
_view) printf '%s' "$2" > "$view_file"; exit 0 ;;
_toggle) if hidden; then printf 0 > "$hidden_file"; else printf 1 > "$hidden_file"; fi; exit 0 ;;
_preview) preview "$2" "$3"; exit 0 ;;
_copy) copy_id "$2"; exit 0 ;;
esac

# --- the popup itself ---

state=$(mktemp -d "${TMPDIR:-/tmp}/scribe-tmux.XXXXXX") || exit 1
SCRIBE_TMUX_STATE=$state; export SCRIBE_TMUX_STATE
trap 'rm -rf "$state"' EXIT
printf sessions > "$state/view"
printf 0 > "$state/hidden"

selection=$(self_call _list-current | fzf \
	--delimiter='	' --with-nth=3 --ansi --no-hscroll --ellipsis= \
	--prompt='scribe> ' \
	--header='enter: open in pager · ctrl-y: copy id · ctrl-s sessions · ctrl-e entries · ctrl-p proposals · ctrl-b backfill · alt-a: show hidden' \
	--preview "$self _preview {1} {2}" \
	--preview-window='right,55%' \
	--bind "ctrl-s:execute-silent($self _view sessions)+reload($self _list-current)" \
	--bind "ctrl-e:execute-silent($self _view entries)+reload($self _list-current)" \
	--bind "ctrl-p:execute-silent($self _view proposals)+reload($self _list-current)" \
	--bind "ctrl-b:execute-silent($self _view transcripts)+reload($self _list-current)" \
	--bind "alt-a:execute-silent($self _toggle)+reload($self _list-current)" \
	--bind "ctrl-y:execute-silent($self _copy {2})") || exit 0
[ -n "$selection" ] || exit 0

kind=$(printf '%s' "$selection" | cut -f1)
id=$(printf '%s' "$selection" | cut -f2)
open_record "$kind" "$id"
