# scribe browse — the popup behind `scribe browse`: sessions, entries,
# proposals and backfill candidates in one fzf list, each record previewed
# with `scribe … show`. It only reads: every command it runs is a list or a
# show. The binary runs this text with `sh -c`, and exports:
#   SCRIBE_BROWSE_BIN  the scribe executable that was started; every child
#                      command runs it, never whichever scribe is on PATH
#   SCRIBE_BROWSE_SH   this text, so fzf's binds and preview can call back in
#
# Keys: enter opens the record (sessions with --log) in $PAGER (else less);
# ctrl-y copies the id (tmux buffer, and pbcopy when present);
# ctrl-s/ctrl-e/ctrl-p/ctrl-b switch to sessions, entries, proposals or
# backfill candidates; alt-a toggles inactive sessions and hidden entries.
set -u

scribe() { "$SCRIBE_BROWSE_BIN" "$@"; }
# `show` prints only its list line when stdout is not a terminal (the
# output contract), so the detail views ask for the table format.
detail() { scribe "$@" --format table; }
# `self` is text for fzf's own shell; the popup's direct call uses the function.
self='sh -c "$SCRIBE_BROWSE_SH" scribe-browse'
self_call() { sh -c "$SCRIBE_BROWSE_SH" scribe-browse "$@"; }
state=${SCRIBE_BROWSE_STATE:-}

view_file=$state/view
hidden_file=$state/hidden

hidden() { [ -f "$hidden_file" ] && [ "$(cat "$hidden_file")" = 1 ]; }

# rows <kind> <list command...>: prefix each tab-separated row with its
# kind so the preview, copy and Enter actions know which noun it is. A
# failed list becomes one notice row (id "-") instead of failing the popup.
rows() {
	kind=$1; shift
	if ! out=$("$@" 2>/dev/null); then
		printf '%s\t-\t%s\n' "$kind" "scribe $kind list is unavailable; run it in a shell for the reason"
		return 0
	fi
	[ -n "$out" ] || return 0
	printf '%s\n' "$out" | awk -v k="$kind" 'BEGIN { OFS = "\t" } { print k, $0 }'
}

list_view() {
	case $1 in
	sessions)
		if hidden; then rows session scribe session list --include-inactive
		else rows session scribe session list; fi ;;
	entries)
		if hidden; then rows entry scribe entry list
		else rows entry scribe entry list --status active; fi ;;
	proposals)
		rows proposal scribe proposal list --state open ;;
	transcripts)
		# Backfill candidates are `transcript list` for the last 30 days.
		# An empty list and a failed list are both one notice row.
		since=$(date -d '30 days ago' +%F 2>/dev/null || date -v-30d +%F 2>/dev/null || date +%F)
		if ! out=$(scribe transcript list --since "$since" 2>/dev/null); then
			printf '%s\n' 'transcript	-	backfill candidates are unavailable: scribe transcript list failed; run it in a shell for the reason'
		elif [ -n "$out" ]; then
			printf '%s\n' "$out" | awk 'BEGIN { OFS = "\t" } { print "transcript", $0 }'
		else
			printf '%s\n' 'transcript	-	no uncataloged transcripts in the last 30 days'
		fi ;;
	esac
}

preview() {
	kind=$1 id=$2
	if [ "$id" = - ]; then
		if [ "$kind" = transcript ]; then
			printf '%s\n' 'No backfill candidate is selected.' \
				'An empty list means no uncataloged transcripts in the window.' \
				'A failed list is this same row; run `scribe transcript list --since <date>` in a shell for the reason.'
		else
			printf '%s list is unavailable; run the list command in a shell for the reason.\n' "$kind"
		fi
		return 0
	fi
	case $kind in
	session) detail session show "$id" ;;
	entry) detail entry show "$id" ;;
	# There is no `proposal show`: the preview formats the proposal's
	# `proposal list` row, which carries every field a proposal has.
	proposal)
		scribe proposal list 2>/dev/null | awk -F '\t' -v id="$id" '
			$1 == id { printf "id: %s\nkind: %s\nstate: %s\nolder: %s\nnewer: %s\nopened: %s\nreason: %s\n", $1, $2, $3, $4, $5, $6, $7; found = 1 }
			END { if (!found) printf "proposal %s not found\n", id }' ;;
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
	case $kind in
	session) detail session show "$id" --log | ${PAGER:-less} ;;
	entry) detail entry show "$id" | ${PAGER:-less} ;;
	proposal) preview proposal "$id" | ${PAGER:-less} ;;
	transcript)
		if [ "$id" = - ]; then preview transcript -; return 0; fi
		# Cataloging runs from a shell, where its progress is visible:
		# Enter leaves the command in the tmux paste buffer and says so.
		# Pasted into that interactive shell, `--author daniel` is
		# Daniel's channel; scribe refuses it through a pipe.
		cmd="scribe session catalog $id --author daniel"
		if command -v tmux >/dev/null 2>&1 && tmux set-buffer -- "$cmd" 2>/dev/null; then
			printf 'scribe browse: paste buffer holds: %s\n' "$cmd"
		else
			printf 'scribe browse: run from a shell: %s\n' "$cmd"
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

state=$(mktemp -d "${TMPDIR:-/tmp}/scribe-browse.XXXXXX") || exit 1
SCRIBE_BROWSE_STATE=$state; export SCRIBE_BROWSE_STATE
trap 'rm -rf "$state"' EXIT
printf sessions > "$state/view"
printf 0 > "$state/hidden"

selection=$(self_call _list-current | fzf \
	--delimiter='	' --with-nth='2..' \
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
