//! Replay fixtures for `scribe session rebuild`: recorded model calls
//! played back by a fake `claude`, and the redaction that turns a private
//! recording into one that can be checked in
//! (docs/design/scribe/specs/replay-fixtures.md).
//!
//! A recording is one directory:
//!
//! - `fixture.json`: `session`, `project` (the directory under
//!   `.claude/projects/` the transcript is placed in), `author`, and
//!   `expect` with the rebuild report's `windows`, `segments` and
//!   `warnings`;
//! - `transcript.jsonl`: the Claude Code transcript;
//! - `store/` (optional): the seed store copied to the data root;
//! - `call-NNN.args`, `call-NNN.in`, `call-NNN.out` per model call, from
//!   `001`: the arguments one per line, the prompt scribe wrote on stdin,
//!   and the stdout the `claude` CLI printed.
//!
//! The fake `claude` answers call `n` with `call-n.out` only when the
//! arguments and prompt equal the recorded ones byte for byte. A request
//! that differs, or a call past the last recorded one, writes the reason to
//! a failure file and exits non-zero; the harness reports it ahead of
//! anything the rebuild says.

use super::{Fixture, copy_private, make_private, text};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Output;

/// The checked-in recordings.
pub fn recordings_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay")
}

/// A recording's `fixture.json`.
#[derive(Clone, Debug)]
pub struct Meta {
    pub session: String,
    pub project: String,
    pub author: String,
    pub expect: Value,
}

impl Meta {
    pub fn read(recording: &Path) -> Result<Self, String> {
        let file = recording.join("fixture.json");
        let raw = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let value: Value =
            serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", file.display()))?;
        let field = |name: &str| {
            value[name]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("{}: `{name}` is not a string", file.display()))
        };
        Ok(Self {
            session: field("session")?,
            project: field("project")?,
            author: field("author")?,
            expect: value["expect"].clone(),
        })
    }
}

/// The recorded calls, `call-001` onwards, stopping at the first gap.
pub fn recorded_calls(recording: &Path) -> usize {
    (1..)
        .take_while(|n| recording.join(format!("{}.out", call_name(*n))).is_file())
        .count()
}

pub fn call_name(n: usize) -> String {
    format!("call-{n:03}")
}

/// How the fake `claude` treats the recorded requests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Answer only a request equal to the recorded one.
    Strict,
    /// Answer in order without comparing; the requests are kept for
    /// [`Run::requests`].
    Record,
}

/// One rebuild driven by a recording.
pub struct Run {
    pub fx: Fixture,
    pub output: Output,
    /// Model calls the rebuild made.
    pub calls: usize,
    /// What the fake `claude` refused, if anything.
    pub replay_failure: Option<String>,
    state: PathBuf,
}

impl Run {
    /// The parsed `--json` report.
    pub fn report(&self) -> Value {
        serde_json::from_slice(&self.output.stdout).unwrap_or(Value::Null)
    }

    /// The request (`.args` and `.in` bytes) the rebuild made as call `n`.
    pub fn request(&self, n: usize) -> (Vec<u8>, Vec<u8>) {
        let read = |ext: &str| {
            std::fs::read(self.state.join(format!("{}.{ext}", call_name(n)))).unwrap_or_default()
        };
        (read("args"), read("in"))
    }

    /// Panic unless the replay consumed every recorded call, refused
    /// nothing and the rebuild exited 0.
    pub fn assert_replayed(&self, recorded: usize) {
        if let Some(failure) = &self.replay_failure {
            panic!(
                "replay refused a request: {failure}\nscribe stderr: {}",
                text(&self.output.stderr)
            );
        }
        assert_eq!(
            self.output.status.code(),
            Some(0),
            "rebuild failed: {}{}",
            text(&self.output.stderr),
            text(&self.output.stdout)
        );
        assert_eq!(
            self.calls, recorded,
            "the rebuild made {} model calls; the recording holds {recorded}",
            self.calls
        );
    }

    /// The written record, through `session show --json`.
    pub fn record(&self, session: &str) -> Value {
        let out = self.fx.ok(&["session", "show", session, "--json"]);
        serde_json::from_slice(&out.stdout).unwrap_or(Value::Null)
    }
}

/// Run `scribe session rebuild <session> --author <author> --confirm --json`
/// through the built binary on a temp root seeded from `recording`, with
/// its transcript under `HOME/.claude/projects/` and the fake `claude`
/// answering from it.
pub fn rebuild(recording: &Path, mode: Mode) -> Run {
    let meta = Meta::read(recording).unwrap_or_else(|e| panic!("{e}"));
    let fx = Fixture::new();
    let store = recording.join("store");
    if store.is_dir() {
        copy_private(&store, &fx.root());
    }
    let project = fx.home().join(".claude/projects").join(&meta.project);
    std::fs::create_dir_all(&project).expect("create projects dir");
    std::fs::copy(
        recording.join("transcript.jsonl"),
        project.join(format!("{}.jsonl", meta.session)),
    )
    .expect("place transcript");
    let state = fx.elsewhere("replay");
    std::fs::create_dir(&state).expect("create replay state");
    let path = fx.fake_program("claude", &fake_claude(recording, &state, mode));
    let out = fx
        .cmd(&[
            "session",
            "rebuild",
            &meta.session,
            "--author",
            &meta.author,
            "--confirm",
            "--json",
        ])
        .env("PATH", path)
        .output()
        .expect("run rebuild");
    let calls = (1..)
        .take_while(|n| state.join(format!("{}.in", call_name(*n))).exists())
        .count();
    let replay_failure = std::fs::read_to_string(state.join("failure")).ok();
    Run {
        fx,
        output: out,
        calls,
        replay_failure,
        state,
    }
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
}

/// The fake `claude`: call `n` saves its arguments and stdin, then prints
/// `call-n.out` when the request matches (or, in [`Mode::Record`], always).
fn fake_claude(recording: &Path, state: &Path, mode: Mode) -> String {
    let strict = mode == Mode::Strict;
    format!(
        r#"#!/bin/sh
rec={rec}
state={state}
n=$(ls "$state" | grep -c '\.in$')
id=$(printf 'call-%03d' $((n + 1)))
cat > "$state/$id.in"
: > "$state/$id.args"
for arg in "$@"; do printf '%s\n' "$arg" >> "$state/$id.args"; done
fail() {{
  printf '%s: %s\n' "$id" "$1" >> "$state/failure"
  printf 'replay: %s: %s\n' "$id" "$1" >&2
  exit 97
}}
[ -f "$rec/$id.out" ] || fail "recording exhausted: no $id.out was recorded"
if [ {strict} = 1 ]; then
  cmp -s "$state/$id.args" "$rec/$id.args" || fail "arguments differ from $id.args"
  cmp -s "$state/$id.in" "$rec/$id.in" || fail "prompt differs from $id.in"
fi
cat "$rec/$id.out"
"#,
        rec = quote(recording),
        state = quote(state),
        strict = u8::from(strict),
    )
}

/// Check a strict replay's outcome against `fixture.json`'s `expect`: the
/// report's `windows`, `segments` and `warnings`, and the record's log.
pub fn assert_expected(recording: &Path, run: &Run) {
    let meta = Meta::read(recording).unwrap_or_else(|e| panic!("{e}"));
    run.assert_replayed(recorded_calls(recording));
    let report = run.report();
    let pass = &report["passes"][0];
    assert_eq!(pass["result"], "written", "{report}");
    for field in ["windows", "segments", "warnings"] {
        assert_eq!(
            pass[field],
            meta.expect[field],
            "{}: {field}",
            recording.display()
        );
    }
    let record = run.record(&meta.session);
    assert_eq!(
        record["log"].as_array().map(Vec::len),
        meta.expect["segments"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok()),
        "written log"
    );
}

/// Rewrite the `.args` and `.in` files of `recording` from a
/// [`Mode::Record`] run, after an intended change to the requests.
pub fn bless(recording: &Path) {
    let run = rebuild(recording, Mode::Record);
    run.assert_replayed(recorded_calls(recording));
    for n in 1..=run.calls {
        let (args, input) = run.request(n);
        let name = call_name(n);
        std::fs::write(recording.join(format!("{name}.args")), args).expect("write args");
        std::fs::write(recording.join(format!("{name}.in")), input).expect("write prompt");
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// A keyed substitution of letters and digits. ASCII letters map to ASCII
/// letters of the same case and digits to digits, by a permutation the key
/// fixes; another alphabetic character maps to a letter of the same UTF-8
/// width (2 bytes: Cyrillic `а`-`я`; 3 bytes: a CJK ideograph). Everything
/// else, whitespace and punctuation included, is kept, so lengths in bytes
/// and characters, word and sentence boundaries and any span relation
/// between two texts survive.
#[derive(Clone, Debug)]
pub struct Key {
    lower: [u8; 26],
    digits: [u8; 10],
    seed: u64,
}

impl Key {
    pub fn new(key: &str) -> Self {
        // FNV-1a, then splitmix64 for the shuffles: stable across platforms.
        let mut seed: u64 = 0xcbf2_9ce4_8422_2325;
        for byte in key.bytes() {
            seed ^= u64::from(byte);
            seed = seed.wrapping_mul(0x0100_0000_01b3);
        }
        let mut state = seed;
        let mut next = move || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = state;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        };
        let mut lower = *b"abcdefghijklmnopqrstuvwxyz";
        let mut digits = *b"0123456789";
        for set in [&mut lower[..], &mut digits[..]] {
            for i in (1..set.len()).rev() {
                let j = usize::try_from(next() % (i as u64 + 1)).unwrap_or(0);
                set.swap(i, j);
            }
        }
        Self {
            lower,
            digits,
            seed,
        }
    }

    pub fn char(&self, c: char) -> char {
        if c.is_ascii_lowercase() {
            char::from(self.lower[usize::from(c as u8 - b'a')])
        } else if c.is_ascii_uppercase() {
            char::from(self.lower[usize::from(c as u8 - b'A')].to_ascii_uppercase())
        } else if c.is_ascii_digit() {
            char::from(self.digits[usize::from(c as u8 - b'0')])
        } else if !c.is_ascii() && c.is_alphabetic() {
            let h = (u64::from(u32::from(c)) ^ self.seed).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 32;
            let pick = |base: u32, span: u64| {
                char::from_u32(base + u32::try_from(h % span).unwrap_or(0)).unwrap_or(c)
            };
            match c.len_utf8() {
                2 => pick(0x0430, 32),
                3 => pick(0x4E00, 0x5000),
                _ => c,
            }
        } else {
            c
        }
    }

    /// Map free text, keeping structure scribe and Claude Code read inside
    /// it: tag names (`<pasted_content`, `</system-reminder>`), the word
    /// after `#` or `/` when it is `scribe`, and a first line that is a
    /// `/scribe` command. `escaped` text is a JSON string body: each escape
    /// sequence is copied as it is.
    pub fn text(&self, s: &str, escaped: bool) -> String {
        let mut out = String::with_capacity(s.len());
        let trimmed = s.trim_start();
        let mut rest = s;
        if trimmed.starts_with("/scribe") {
            let lead = s.len() - trimmed.len();
            let end = if escaped {
                trimmed.find("\\n")
            } else {
                trimmed.find('\n')
            }
            .map_or(s.len(), |i| lead + i);
            out.push_str(&s[..end]);
            rest = &s[end..];
        }
        let mut chars = rest.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            if escaped && c == '\\' {
                out.push(c);
                if let Some((_, e)) = chars.next() {
                    out.push(e);
                    if e == 'u' {
                        for _ in 0..4 {
                            if let Some((_, h)) = chars.next() {
                                out.push(h);
                            }
                        }
                    }
                }
                continue;
            }
            if c == '<' || c == '#' || c == '/' {
                out.push(c);
                let after = &rest[i + 1..];
                let kept = if c == '<' {
                    let name = after.strip_prefix('/').unwrap_or(after);
                    let len = name
                        .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'))
                        .unwrap_or(name.len());
                    if name.starts_with(|ch: char| ch.is_ascii_alphabetic()) {
                        after.len() - name.len() + len
                    } else {
                        0
                    }
                } else if after.starts_with("scribe")
                    && !after[6..].starts_with(|ch: char| ch.is_alphanumeric())
                {
                    6
                } else {
                    0
                };
                for _ in 0..after[..kept].chars().count() {
                    if let Some((_, k)) = chars.next() {
                        out.push(k);
                    }
                }
                continue;
            }
            out.push(self.char(c));
        }
        out
    }
}

/// JSON keys whose string values are ids, enums, timestamps or references,
/// kept as they are. Every other string value is free text and mapped.
const KEEP_KEYS: &[&str] = &[
    "type",
    "subtype",
    "role",
    "uuid",
    "parentUuid",
    "logicalParentUuid",
    "leafUuid",
    "sessionId",
    "session_id",
    "timestamp",
    "version",
    "userType",
    "requestId",
    "id",
    "tool_use_id",
    "name",
    "model",
    "stop_reason",
    "stop_sequence",
    "signature",
    "gitBranch",
    "level",
    "service_tier",
    "starts_at",
    "message",
    "related",
    "status",
    "state",
    "origin",
    "kind",
    "since",
    "at",
    "retry_after",
    "pending_links",
    "cite",
    "target",
    "command_id",
    "media_type",
];

/// A string value that is an id or a timestamp whatever its key.
fn looks_like_id(s: &str) -> bool {
    let uuid = s.len() == 36
        && s.char_indices().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        });
    let timestamp = s.len() >= 20
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[10] == b'T'
        && s[..4].bytes().all(|b| b.is_ascii_digit());
    uuid || timestamp
}

/// Redact one JSON document (or a JSONL line) without reparsing it: keys,
/// numbers, literals, whitespace and escapes stay byte for byte; a string
/// value is mapped unless its nearest key is in [`KEEP_KEYS`] or it looks
/// like an id or timestamp. Text that is not JSON is mapped whole.
pub fn redact_json(key: &Key, src: &str) -> String {
    enum Ctx {
        Object { key: String, want_key: bool },
        Array { key: String },
    }
    if serde_json::from_str::<Value>(src.trim()).is_err() {
        return key.text(src, false);
    }
    let mut out = String::with_capacity(src.len());
    let mut stack: Vec<Ctx> = Vec::new();
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match b {
            b'{' => {
                stack.push(Ctx::Object {
                    key: String::new(),
                    want_key: true,
                });
                out.push('{');
                i += 1;
            }
            b'[' => {
                let parent = match stack.last() {
                    Some(Ctx::Object { key, .. } | Ctx::Array { key }) => key.clone(),
                    None => String::new(),
                };
                stack.push(Ctx::Array { key: parent });
                out.push('[');
                i += 1;
            }
            b'}' | b']' => {
                stack.pop();
                out.push(char::from(b));
                i += 1;
            }
            b',' => {
                if let Some(Ctx::Object { want_key, .. }) = stack.last_mut() {
                    *want_key = true;
                }
                out.push(',');
                i += 1;
            }
            b'"' => {
                let mut end = i + 1;
                while end < bytes.len() && bytes[end] != b'"' {
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                let body = &src[i + 1..end];
                let is_key = matches!(stack.last(), Some(Ctx::Object { want_key: true, .. }));
                out.push('"');
                if is_key {
                    if let Some(Ctx::Object { key, want_key }) = stack.last_mut() {
                        body.clone_into(key);
                        *want_key = false;
                    }
                    out.push_str(body);
                } else {
                    let owner = match stack.last() {
                        Some(Ctx::Object { key, .. } | Ctx::Array { key }) => key.as_str(),
                        None => "",
                    };
                    if KEEP_KEYS.contains(&owner) || looks_like_id(body) {
                        out.push_str(body);
                    } else if let Some(embedded) = redact_embedded(key, body) {
                        out.push_str(&embedded);
                    } else {
                        out.push_str(&key.text(body, true));
                    }
                }
                out.push('"');
                i = end + 1;
            }
            _ => {
                let len = src[i..].chars().next().map_or(1, char::len_utf8);
                out.push_str(&src[i..i + len]);
                i += len;
            }
        }
    }
    out
}

/// A string value holding a JSON object, optionally in a ```` ```json ````
/// fence: the CLI's `result` text when it printed no `structured_output`.
/// Its keys are kept and its values redacted like any document; the result
/// is escaped again as a JSON string body.
fn redact_embedded(key: &Key, body: &str) -> Option<String> {
    let decoded: String = serde_json::from_str(&format!("\"{body}\"")).ok()?;
    let trimmed = decoded.trim();
    let inner = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```"))
        .and_then(|b| b.strip_suffix("```"))
        .unwrap_or(trimmed)
        .trim();
    if !inner.starts_with('{') || serde_json::from_str::<Value>(inner).is_err() {
        return None;
    }
    let at = decoded.find(inner)?;
    let rewritten = format!(
        "{}{}{}",
        &decoded[..at],
        redact_json(key, inner),
        &decoded[at + inner.len()..]
    );
    let encoded = serde_json::to_string(&rewritten).ok()?;
    Some(encoded[1..encoded.len() - 1].to_owned())
}

/// Frontmatter keys of store Markdown whose values are free text.
const FREE_YAML_KEYS: &[&str] = &[
    "title", "heading", "term", "said", "reason", "cwd", "project", "host", "summary", "goals",
    "text", "note", "question", "answer", "content",
];
/// Frontmatter keys whose list items are free text.
const FREE_YAML_LISTS: &[&str] = &["topics"];

/// Redact a store Markdown file: in the frontmatter, the values of
/// [`FREE_YAML_KEYS`] (block scalars included) and the items of
/// [`FREE_YAML_LISTS`]; in the body, every line but `##` section headings,
/// keeping a `### <n>. ` segment prefix. YAML escapes in a quoted value are
/// copied as they are.
pub fn redact_markdown(key: &Key, src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut fences = 0;
    // (indent, key) of the enclosing frontmatter mapping keys.
    let mut keys: Vec<(usize, String)> = Vec::new();
    // A block scalar of a free key: lines indented past this are mapped.
    let mut block: Option<usize> = None;
    for line in src.split_inclusive('\n') {
        let body = line.trim_end_matches('\n');
        let newline = &line[body.len()..];
        if body == "---" && fences < 2 {
            fences += 1;
            out.push_str(line);
            continue;
        }
        if fences == 1 {
            let indent = body.len() - body.trim_start().len();
            if let Some(at) = block {
                if indent > at || body.trim().is_empty() {
                    out.push_str(&key.text(body, false));
                    out.push_str(newline);
                    continue;
                }
                block = None;
            }
            let item = body.trim_start().strip_prefix("- ");
            let entry = item.unwrap_or(body.trim_start());
            // The column the entry's key sits at, past any `- `.
            let column = indent + if item.is_some() { 2 } else { 0 };
            let pair = entry.split_once(": ").or_else(|| {
                entry
                    .strip_suffix(':')
                    .map(|k| (k, ""))
                    .filter(|(k, _)| !k.contains(' '))
            });
            let simple_key = pair.filter(|(k, _)| {
                !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
            match simple_key {
                Some((name, value)) => {
                    keys.retain(|(at, _)| *at < column);
                    keys.push((column, name.to_owned()));
                    let prefix_len = body.len() - value.len();
                    out.push_str(&body[..prefix_len]);
                    if FREE_YAML_KEYS.contains(&name) {
                        if matches!(value.trim(), "|" | "|-" | "|+" | ">" | ">-" | ">+") {
                            out.push_str(value);
                            block = Some(column);
                        } else {
                            out.push_str(&key.text(value, value.starts_with('"')));
                        }
                    } else {
                        out.push_str(value);
                    }
                }
                None if item.is_some() => {
                    keys.retain(|(at, _)| *at <= indent);
                    let owner = keys.last().map(|(_, k)| k.as_str()).unwrap_or_default();
                    let value = item.unwrap_or_default();
                    out.push_str(&body[..body.len() - value.len()]);
                    if FREE_YAML_LISTS.contains(&owner) {
                        out.push_str(&key.text(value, value.starts_with('"')));
                    } else {
                        out.push_str(value);
                    }
                }
                None => out.push_str(body),
            }
            out.push_str(newline);
            continue;
        }
        if fences == 0 {
            out.push_str(line);
            continue;
        }
        if body.starts_with("## ") {
            out.push_str(line);
        } else if let Some(rest) = body.strip_prefix("### ") {
            let number = rest
                .find(". ")
                .filter(|at| rest[..*at].bytes().all(|b| b.is_ascii_digit()))
                .map_or(0, |at| at + 2);
            out.push_str("### ");
            out.push_str(&rest[..number]);
            out.push_str(&key.text(&rest[number..], false));
            out.push_str(newline);
        } else {
            out.push_str(&key.text(body, false));
            out.push_str(newline);
        }
    }
    out
}

/// Redact one recording file by its kind. `.args` and TOML are kept.
fn redact_file(key: &Key, name: &str, src: &str) -> String {
    if name.ends_with(".md") {
        redact_markdown(key, src)
    } else if name.ends_with(".jsonl") {
        src.split_inclusive('\n')
            .map(|line| {
                let body = line.trim_end_matches('\n');
                format!("{}{}", redact_json(key, body), &line[body.len()..])
            })
            .collect()
    } else if name.ends_with(".json") || name.ends_with(".out") {
        redact_json(key, src)
    } else {
        src.to_owned()
    }
}

/// Redact `from` into `to` (which must not exist) with `key`, in one step:
///
/// 1. map the transcript, the seed store, the model outputs and the
///    project directory name; keep `.args`, `fixture.json`'s ids and
///    expectations, and the store's config;
/// 2. replay the redacted recording in [`Mode::Record`] and write the
///    prompts scribe sends for it as the new `call-NNN.in`;
/// 3. check every new prompt has the original's length and, at each
///    character, either the original character or its mapped one (the
///    prompt template stays, the data is mapped), and that the arguments
///    are unchanged;
/// 4. replay the result strictly and check `fixture.json`'s expectations.
///
/// Any check that fails removes nothing and returns why; `to` then holds
/// the partial output for inspection.
pub fn redact_recording(from: &Path, to: &Path, key: &Key) -> Result<(), String> {
    if to.exists() {
        return Err(format!("{} already exists", to.display()));
    }
    let meta = Meta::read(from)?;
    let calls = recorded_calls(from);
    copy_mapped(from, to, key, true)?;
    let fixture_file = to.join("fixture.json");
    let mut fixture: Value = serde_json::from_slice(
        &std::fs::read(from.join("fixture.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fixture["project"] = Value::String(key.text(&meta.project, false));
    std::fs::write(
        &fixture_file,
        serde_json::to_string_pretty(&fixture).map_err(|e| e.to_string())? + "\n",
    )
    .map_err(|e| e.to_string())?;

    let run = rebuild(to, Mode::Record);
    if let Some(failure) = &run.replay_failure {
        return Err(format!("redacted replay: {failure}"));
    }
    if run.calls != calls {
        return Err(format!(
            "the redacted rebuild made {} model calls; the recording holds {calls}: {}",
            run.calls,
            text(&run.output.stderr)
        ));
    }
    for n in 1..=calls {
        let name = call_name(n);
        let (args, prompt) = run.request(n);
        let original_args =
            std::fs::read(from.join(format!("{name}.args"))).map_err(|e| e.to_string())?;
        if args != original_args {
            return Err(format!("{name}: the arguments changed"));
        }
        let original = std::fs::read_to_string(from.join(format!("{name}.in")))
            .map_err(|e| format!("{name}.in: {e}"))?;
        let prompt = String::from_utf8(prompt).map_err(|e| e.to_string())?;
        check_mapped(key, &original, &prompt).map_err(|e| format!("{name}.in: {e}"))?;
        std::fs::write(to.join(format!("{name}.in")), prompt).map_err(|e| e.to_string())?;
    }
    let strict = rebuild(to, Mode::Strict);
    if let Some(failure) = &strict.replay_failure {
        return Err(format!(
            "strict replay of the redacted recording: {failure}"
        ));
    }
    let report = strict.report();
    for field in ["windows", "segments", "warnings"] {
        if report["passes"][0][field] != fixture["expect"][field] {
            return Err(format!(
                "the redacted rebuild's {field} is {}, expected {}",
                report["passes"][0][field], fixture["expect"][field]
            ));
        }
    }
    Ok(())
}

/// The prompt lines that list store labels in sorted order, and their
/// separators: mapping reorders them, so they compare as sets.
const SORTED_LINES: &[(&str, &str)] = &[("Existing topics: ", ", "), ("Existing terms: ", "; ")];

/// `prompt` is `original` with some characters mapped by `key`: line for
/// line, each character is the recorded one or its mapped one, except that
/// a [`SORTED_LINES`] line holds the mapped items in any order.
pub fn check_mapped(key: &Key, original: &str, prompt: &str) -> Result<(), String> {
    let a: Vec<&str> = original.split('\n').collect();
    let b: Vec<&str> = prompt.split('\n').collect();
    if a.len() != b.len() {
        return Err(format!(
            "the recorded prompt has {} lines, the redacted run sent {}",
            a.len(),
            b.len()
        ));
    }
    for (n, (o, p)) in a.iter().zip(&b).enumerate() {
        let line = n + 1;
        if let Some((prefix, sep)) = SORTED_LINES
            .iter()
            .find(|(prefix, _)| o.starts_with(prefix))
        {
            let mut want: Vec<String> = o[prefix.len()..]
                .split(sep)
                .map(|item| key.text(item, false))
                .collect();
            let mut got: Vec<String> = p
                .strip_prefix(prefix)
                .ok_or_else(|| format!("line {line}: expected {prefix:?}"))?
                .split(sep)
                .map(str::to_owned)
                .collect();
            want.sort();
            got.sort();
            if want != got {
                return Err(format!("line {line}: the listed labels differ"));
            }
            continue;
        }
        let mut oc = o.chars();
        let mut pc = p.chars();
        let mut at = 0usize;
        loop {
            match (oc.next(), pc.next()) {
                (None, None) => break,
                (Some(x), Some(y)) if y == x || y == key.char(x) => at += 1,
                (x, y) => {
                    return Err(format!(
                        "line {line}, character {at}: recorded {x:?}, redacted run sent {y:?}"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Copy the tree at `from` to `to` with every file redacted, skipping the
/// `.in` prompts at the top level (they are rewritten from a replay).
fn copy_mapped(from: &Path, to: &Path, key: &Key, top: bool) -> Result<(), String> {
    std::fs::create_dir(to).map_err(|e| format!("{}: {e}", to.display()))?;
    make_private(to);
    let mut items: Vec<_> = std::fs::read_dir(from)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .collect();
    items.sort_by_key(std::fs::DirEntry::file_name);
    for item in items {
        let name = item.file_name().to_string_lossy().into_owned();
        let target = to.join(&name);
        if item.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_mapped(&item.path(), &target, key, false)?;
            continue;
        }
        if top && (name.ends_with(".in") || name == "fixture.json") {
            continue;
        }
        let src = std::fs::read_to_string(item.path())
            .map_err(|e| format!("{}: {e}", item.path().display()))?;
        std::fs::write(&target, redact_file(key, &name, &src)).map_err(|e| e.to_string())?;
        make_private(&target);
    }
    Ok(())
}
