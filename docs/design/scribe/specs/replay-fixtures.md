# Session rebuild replay fixtures

Three rebuild fixes in a row passed their own tests and then failed on a
real long session: the tests never held the inputs that fail. A replay
fixture is a recording of one session rebuild, its transcript and every
model call with the answer the `claude` CLI printed, that the tests play
back to the `scribe-session-rebuild` job's plugin steps in the built binary
with no live model, so the outcome does not depend on what the model
returns on the day
([2_design.md §5.6](../2_design.md#56-rebuilding-a-record)).

Code: `crates/scribe/tests/support/replay.rs` (harness),
`crates/scribe/tests/support/replay/redact.rs` (redaction),
`crates/scribe/tests/session_replay.rs` (the tests),
`crates/scribe/tests/fixtures/replay/` (the recordings),
`scripts/redact-recording.sh` (the redaction tool). None of it is part of
the shipped CLI.

## Recording format

One directory per recording:

| Path | Holds |
| --- | --- |
| `fixture.json` | `session` (the session id), `project` (the directory under `HOME/.claude/projects/` the transcript is placed in), `author` (`--author`), and `expect`: the rebuild report's `windows`, `segments` and `warnings` |
| `transcript.jsonl` | The Claude Code transcript, as Claude Code wrote it |
| `store/` | Optional seed store, copied owner-only to the data root: the record to rebuild, `config.toml` (window sizes), `control.json` |
| `call-NNN.args` | The activity call `NNN` goes to, from `001`: `activity:scribe_session_distill` or `activity:scribe_session_header_compress` |
| `call-NNN.in` | The prompt the activity renders from scribe's request (`SessionWindowInput::prompt` or `SessionHeaderRequest::prompt`) |
| `call-NNN.out` | The stdout the `claude` CLI printed for it: the JSON envelope with `structured_output`, or a `result` text holding the JSON |

The recorded calls are `call-001` up to the first missing `.out`.

## The replay

The test copies the seed store to a temp root, places the transcript under
the temp `HOME/.claude/projects/<project>/<session>.jsonl` and runs the
`scribe-session-rebuild` job's steps (`scribe.session-rebuild-prepare`,
`-apply`, the one targeted retry, `-finish`) through `scribe plugin call`,
as the job does, for the recording's session and author with `confirm:
true`. Each activity call `n` is answered with `call-n.out`'s
`structured_output` (or its `result` text parsed as JSON), only when its
activity and prompt equal `call-n.args` and `call-n.in` byte for byte. A
request that differs, or a call past the last recording, is written to a
failure file and the activity fails, which fails the run; the test
reports that reason first. The recordings were made against the
in-process rebuild ([ORB-28143] moved it to the job): re-blessing for the
job rewrote only the `.args`, and every `.in` prompt stayed byte for
byte. A replay passes only when nothing was refused, the run finished,
it made exactly the recorded number of calls, and the report and the
written log match `expect`.

`SCRIBE_REPLAY_UPDATE=1 cargo test -p scribe --test session_replay`
rewrites every recording's `.args` and `.in` from the requests the current
build makes. Run it only after an intended prompt change and review the
diff as a contract change, as with `make goldens UPDATE=1`: the recorded
answers were given to the old prompts.

## The synthetic recording

`fixtures/replay/synthetic-rebuild/` is hand-written (no real conversation)
and reproduces each failure seen on the real long session, five windows of
two messages and nine calls:

| Window | Calls | Failure | Repair warning |
| --- | --- | --- | --- |
| 1 | 1–2 | An unexpected `open` with no open segment; `next` at 1003 of 1000, still 1167 after the header-only compression | `repaired next: cut at bullet boundary` |
| 2 | 3–4 | `open` omitted (keep the open segment); new segment texts of 904 and 924 of 900, repeated on retry | `repaired new segment 1 text` and `2 text`, sentence boundary |
| 3 | 5–6 | `starts_at` at the open segment's last message, where continuing the open segment would overflow 900; the retry answers in `result` text | `opened at the first window message after the open segment` |
| 4 | 7–8 | `starts_at` before the open segment's last message, a later start kept, so no message can hold it | `continued the open segment, cut at sentence boundary` |
| 5 | 9 | A clean final answer | none |

The transcript also carries the lines a pass skips: a `summary` line, a
`/scribe on` turn, a slash-command turn, tool calls and results, thinking
and a meta line, and a turn with a `<pasted_content>` block.

## Redacting a real recording

Real recordings hold Daniel's words; whether a redacted copy is committed
is Daniel's call. Redacting one is one command:

```sh
scripts/redact-recording.sh <recording-dir> <output-dir> < key-file
```

The key is one line on stdin: a terminal gets a silent prompt, and a file
or pipe supplies it otherwise. It is never an argument, because `ps` lists
arguments for every user. The script runs the `redact_a_recording` test with
`SCRIBE_REDACT_FROM` and `SCRIBE_REDACT_TO` set, and the key on that test's
stdin. Without them that test redacts the synthetic recording into a temp
directory and checks the result. Every step runs on temp roots; nothing
reads or writes `~/.scribe`.

1. Every free text in the transcript, the seed store and the model outputs
   is rewritten by a substitution `<key>` fixes: an ASCII letter becomes an
   ASCII letter of the same case, a digit a digit, by one permutation each;
   a cased non-ASCII letter becomes a letter of the same case and UTF-8
   width; a letter from a script without case becomes a same-width letter
   from another script. The project
   directory name is mapped too.
2. The replay of the redacted recording, answering in order without
   comparing, gives the prompts scribe sends for it; they become the new
   `call-NNN.in`.
3. Each new prompt must equal the recorded one at every character or be its
   mapped character (the prompt template stays, the data is mapped), with
   the same lines; the `Existing topics:` and `Existing terms:` lines, which
   scribe sorts, compare as sets. The arguments must be unchanged.
4. The result is replayed strictly and must meet `fixture.json`'s `expect`.

Any failed check stops the tool with the call, line and character.

**Preserved:** every file's name and length in bytes and characters;
whitespace, punctuation, line breaks and Markdown bullet and heading
structure; letter case (ASCII and cased non-ASCII); JSON keys, numbers, literals and escape
sequences; string values under id, enum and reference keys (`uuid`,
`parentUuid`, `sessionId`, `timestamp`, `type`, `role`, `id`, `name`,
`model`, `starts_at`, `message`, `related`, `status`, `state`, `kind`,
...), and any string that is a UUID or an RFC 3339 timestamp; tag names
(`<pasted_content>`, `<system-reminder>`), a first line that is a
`/scribe` command and the word `scribe` after `#` or `/`; in store
Markdown, the frontmatter keys and every value but free text (`title`,
`heading`, `term`, `said`, `reason`, `cwd`, `project`, `host`, topic
items), `##` section headings and the `### <n>. ` segment prefix; the
store's `config.toml`; `fixture.json` but `project`. Because the mapping is
per character, a `said` that is a span of a message stays a span of the
mapped message, term matches still hold, and character budgets, word and
sentence cuts and the repairs they cause come out the same.

**Not preserved or not hidden:**

- It is a substitution, not encryption. Anyone with the key inverts it,
  and a long English text can be decoded by letter frequency without it.
  Keep the key out of the repository; treat the output as obscured, not
  anonymous.
- Kept values stay readable: ids, timestamps, model names, tool names,
  `related` references (`task:<id>`, `repo:<slug>`), tag names, git branch
  names and the word `scribe` in paths.
- Escaped characters in JSON (`é`) and in YAML double-quoted values
  are kept as they are.
- A mapped value may read as a YAML or TOML literal (`true`, a number);
  the replay then fails loudly rather than producing a wrong fixture.
- More than 200 store topics or terms: scribe sends the first 200 in
  sorted order, which mapping changes, so step 3 refuses the recording.
