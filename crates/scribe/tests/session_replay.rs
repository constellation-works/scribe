//! `scribe session rebuild` against recorded model answers
//! (docs/design/scribe/specs/replay-fixtures.md): every recording under
//! `tests/fixtures/replay/` is rebuilt through the built binary on a temp
//! root, a fake `claude` replaying its calls in order, with no live model.
//! The replay refuses a request that differs from the recorded one and a
//! call past the last recorded one.
//!
//! `SCRIBE_REPLAY_UPDATE=1` rewrites each recording's `.args` and `.in`
//! from the requests the current build makes, after an intended prompt
//! change; review that diff as a contract change, like the goldens.
//!
//! `redact_a_recording` is also the redaction tool: with
//! `SCRIBE_REDACT_FROM` and `SCRIBE_REDACT_TO` set and the key on stdin
//! (`scripts/redact-recording.sh`), it redacts that recording instead of
//! the synthetic one.
#![allow(clippy::expect_used, clippy::unwrap_used)]

mod support;

use serde_json::Value;
use std::path::{Path, PathBuf};
use support::replay::{self, Key, Mode};
use support::{Fixture, copy_private};

fn synthetic() -> PathBuf {
    replay::recordings_dir().join("synthetic-rebuild")
}

fn recordings() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(replay::recordings_dir())
        .unwrap()
        .map(|item| item.unwrap().path())
        .filter(|path| path.join("fixture.json").is_file())
        .collect();
    dirs.sort();
    dirs
}

/// A writable copy of `recording` in `fx`.
fn copy(fx: &Fixture, recording: &Path) -> PathBuf {
    let to = fx.elsewhere("recording");
    copy_private(recording, &to);
    to
}

#[test]
fn every_checked_in_recording_rebuilds_from_its_replayed_model_answers() {
    let update = std::env::var_os("SCRIBE_REPLAY_UPDATE").is_some_and(|v| v == "1");
    let all = recordings();
    assert!(all.contains(&synthetic()), "{all:?}");
    for recording in all {
        if update {
            replay::bless(&recording);
        }
        let run = replay::rebuild(&recording, Mode::Strict);
        replay::assert_expected(&recording, &run);
    }
}

#[test]
fn the_synthetic_recording_reproduces_each_historical_rebuild_failure() {
    let recording = synthetic();
    let run = replay::rebuild(&recording, Mode::Strict);
    replay::assert_expected(&recording, &run);
    let meta = replay::Meta::read(&recording).unwrap();

    // Each failure the recording holds is reported as its repair.
    let warnings = run.report()["passes"][0]["warnings"].clone();
    assert_eq!(
        warnings,
        serde_json::json!([
            "window 1: repaired next: cut at bullet boundary (budget 1000 characters)",
            "window 2: repaired new segment 1 text: cut at sentence boundary (budget 900 characters)",
            "window 2: repaired new segment 2 text: cut at sentence boundary (budget 900 characters)",
            "window 3: repaired new segment 1 starts_at: opened at the first window message after the open segment",
            "window 4: repaired new segment 1 starts_at: continued the open segment, cut at sentence boundary (budget 900 characters)"
        ])
    );
    let out = |n: usize| -> Value {
        let raw = std::fs::read(recording.join(format!("{}.out", replay::call_name(n)))).unwrap();
        serde_json::from_slice::<Value>(&raw).unwrap()["structured_output"].clone()
    };
    let rendered_next = |answer: &Value| {
        let items = answer["next"].as_array().unwrap();
        items
            .iter()
            .map(|i| i.as_str().unwrap().chars().count() + 2)
            .sum::<usize>()
            + items.len()
            - 1
    };
    let chars = |v: &Value| v.as_str().unwrap().chars().count();
    // Window 1: an unexpected open segment, and `next` over budget before
    // (1003) and after (1167) the header-only compression.
    assert!(out(1)["open"].is_object());
    assert_eq!(rendered_next(&out(1)), 1003);
    assert_eq!(rendered_next(&out(2)), 1167);
    let (_, compress) = run.request(2);
    assert!(compress.starts_with(b"Compress only"));
    // Window 2: the open segment omitted, new segment texts of 904 and 924.
    assert!(out(3).get("open").is_none());
    assert_eq!(chars(&out(3)["new"][0]["text"]), 904);
    assert_eq!(chars(&out(3)["new"][1]["text"]), 924);
    // Windows 3 and 4: starts at and before the open segment's last message,
    // where continuing the open segment would overflow it.
    let transcript = std::fs::read_to_string(recording.join("transcript.jsonl")).unwrap();
    let ids: Vec<String> = transcript
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .filter(|l| l["message"]["role"].is_string())
        .map(|l| l["uuid"].as_str().unwrap().to_owned())
        .collect();
    let record = run.record(&meta.session);
    let log = record["log"].as_array().unwrap();
    assert_eq!(out(5)["new"][0]["starts_at"], log[3]["last_message"]);
    assert!(
        chars(&out(5)["open"]["text"]) + 2 + chars(&out(5)["new"][0]["text"]) > 900,
        "window 3 continuation overflows"
    );
    let at = |id: &Value| ids.iter().position(|i| i == id.as_str().unwrap()).unwrap();
    assert!(at(&out(7)["new"][0]["starts_at"]) < at(&log[4]["last_message"]));
    // The record: the unexpected open became the first segment; every
    // segment and the header are within budget; all are rebuilt and sealed.
    assert_eq!(
        log[0]["first_message"], ids[2],
        "first conversation message"
    );
    for segment in log {
        assert_eq!(segment["origin"], "rebuild");
        assert_eq!(segment["state"], "sealed");
    }
    let shown = run
        .fx
        .ok(&["session", "show", &meta.session, "--log", "--json"]);
    let shown: Value = serde_json::from_slice(&shown.stdout).unwrap();
    for segment in shown["log"].as_array().unwrap() {
        assert!(chars(&segment["text"]) <= 900, "{segment}");
    }
    assert!(chars(&record["next"]) <= 1000);
    assert_eq!(
        record["scribed_through"],
        Value::from(ids.last().unwrap().as_str())
    );
}

#[test]
fn a_request_that_differs_from_the_recording_fails_the_replay() {
    for (file, reason) in [
        ("call-003.in", "call-003: prompt differs from call-003.in"),
        (
            "call-001.args",
            "call-001: arguments differ from call-001.args",
        ),
    ] {
        let fx = Fixture::new();
        let recording = copy(&fx, &synthetic());
        let path = recording.join(file);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.extend_from_slice(b"one more line\n");
        std::fs::write(&path, bytes).unwrap();

        let run = replay::rebuild(&recording, Mode::Strict);

        assert_eq!(run.replay_failure.as_deref(), Some(&*format!("{reason}\n")));
        assert_eq!(run.output.status.code(), Some(1), "the rebuild fails");
        assert_eq!(run.report()["passes"][0]["error_code"], "model_failed");
        let meta = replay::Meta::read(&recording).unwrap();
        assert_eq!(
            run.record(&meta.session)["log"].as_array().unwrap().len(),
            1,
            "the seed record is kept"
        );
    }
}

#[test]
fn an_exhausted_recording_fails_the_replay() {
    let fx = Fixture::new();
    let recording = copy(&fx, &synthetic());
    let last = replay::recorded_calls(&recording);
    let name = replay::call_name(last);
    for ext in ["args", "in", "out"] {
        std::fs::remove_file(recording.join(format!("{name}.{ext}"))).unwrap();
    }

    let run = replay::rebuild(&recording, Mode::Strict);

    assert_eq!(
        run.replay_failure.as_deref(),
        Some(&*format!(
            "{name}: recording exhausted: no {name}.out was recorded\n"
        ))
    );
    assert_eq!(run.calls, last);
    assert_eq!(run.output.status.code(), Some(1));
}

/// Every file under `root`, relative, with its bytes.
fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    support::snapshot(root)
        .into_iter()
        .filter(|(path, _, _)| path.is_file())
        .map(|(path, bytes, _)| (path.strip_prefix(root).unwrap().to_path_buf(), bytes))
        .collect()
}

/// The redaction key: one line on stdin, which `scripts/redact-recording.sh`
/// writes. It is never in argv or the environment (STD-05 R12).
fn redaction_key_from_stdin() -> String {
    let mut line = String::new();
    std::io::stdin()
        .read_line(&mut line)
        .expect("read the redaction key from stdin");
    line.trim_end_matches(['\r', '\n']).to_owned()
}

/// The redaction tool. Without `SCRIBE_REDACT_FROM` it redacts the synthetic
/// recording into a temp directory and checks what the redaction keeps; with
/// it, it redacts the recording it names, to `SCRIBE_REDACT_TO`, under the
/// key read from stdin.
#[test]
fn redact_a_recording() {
    let fx = Fixture::new();
    let named = std::env::var_os("SCRIBE_REDACT_FROM").map(PathBuf::from);
    if let Some(from) = named {
        let to = std::env::var_os("SCRIBE_REDACT_TO").expect("SCRIBE_REDACT_TO");
        let key = redaction_key_from_stdin();
        assert!(!key.is_empty(), "the redaction key on stdin is empty");
        replay::redact_recording(&from, Path::new(&to), &Key::new(&key))
            .unwrap_or_else(|e| panic!("redaction refused: {e}"));
        return;
    }
    let from = synthetic();
    let key = Key::new("replay-test-key");

    // The substitution preserves case and UTF-8 width for non-ASCII text,
    // including scripts whose characters use three or four bytes.
    let sample = "ÉéЯяᲐა𐐀𐐨ǅᾈ中";
    let mapped = key.text(sample, false);
    assert_eq!(mapped.len(), sample.len());
    for (before, after) in sample.chars().zip(mapped.chars()) {
        let before_upper: String = before.to_uppercase().collect();
        let before_lower: String = before.to_lowercase().collect();
        let after_upper: String = after.to_uppercase().collect();
        let after_lower: String = after.to_lowercase().collect();
        if before_upper != before_lower {
            assert_ne!(before, after);
            assert_ne!(after_upper, after_lower);
        }
        assert_eq!(before.len_utf8(), after.len_utf8());
        assert_eq!(before.is_uppercase(), after.is_uppercase());
        assert_eq!(before.is_lowercase(), after.is_lowercase());
    }

    let to = fx.elsewhere("redacted");
    replay::redact_recording(&from, &to, &key).unwrap();

    // Deterministic: the same key gives the same bytes, another key others.
    let again = fx.elsewhere("again");
    replay::redact_recording(&from, &again, &key).unwrap();
    assert_eq!(tree(&to), tree(&again));
    let other = fx.elsewhere("other");
    replay::redact_recording(&from, &other, &Key::new("another-key")).unwrap();
    assert_ne!(tree(&to), tree(&other));

    // Every file keeps its name and byte length; the free text is gone.
    let original = tree(&from);
    let redacted = tree(&to);
    assert_eq!(
        original
            .iter()
            .map(|(p, b)| (p, b.len()))
            .collect::<Vec<_>>(),
        redacted
            .iter()
            .map(|(p, b)| (p, b.len()))
            .collect::<Vec<_>>()
    );
    for (path, bytes) in &redacted {
        let text = String::from_utf8_lossy(bytes);
        for phrase in [
            "replay fixture",
            "café",
            "Daniel defined",
            "Planning replay",
        ] {
            assert!(
                !text.contains(phrase),
                "{} still holds {phrase:?}",
                path.display()
            );
        }
    }

    // Ids, timestamps, keys and enums are kept, line for line.
    let lines = |dir: &Path| -> Vec<Value> {
        std::fs::read_to_string(dir.join("transcript.jsonl"))
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let (before, after) = (lines(&from), lines(&to));
    assert_eq!(before.len(), after.len());
    for (b, a) in before.iter().zip(&after) {
        let keys = |v: &Value| v.as_object().unwrap().keys().cloned().collect::<Vec<_>>();
        assert_eq!(keys(b), keys(a));
        for field in [
            "uuid",
            "parentUuid",
            "sessionId",
            "timestamp",
            "type",
            "leafUuid",
        ] {
            assert_eq!(b[field], a[field], "{field}");
        }
        assert_eq!(b["message"]["role"], a["message"]["role"]);
    }

    // Verbatim spans survive: each term's `said` is still in its message.
    let message_text = |dir: &Path, id: &str| -> String {
        let line = lines(dir).into_iter().find(|l| l["uuid"] == id).unwrap();
        line["message"]["content"].as_str().unwrap().to_owned()
    };
    let last = replay::call_name(replay::recorded_calls(&to));
    let answer: Value =
        serde_json::from_slice(&std::fs::read(to.join(format!("{last}.out"))).unwrap()).unwrap();
    let terms = answer["structured_output"]["terms"].as_array().unwrap();
    assert_eq!(terms.len(), 2);
    for term in terms {
        let said = term["said"].as_str().unwrap();
        assert!(message_text(&to, term["message"].as_str().unwrap()).contains(said));
        assert!(!message_text(&from, term["message"].as_str().unwrap()).contains(said));
    }

    // The redacted recording replays strictly with the same outcome.
    let run = replay::rebuild(&to, Mode::Strict);
    replay::assert_expected(&to, &run);
}

#[test]
fn redaction_refuses_a_destination_inside_its_source() {
    let fx = Fixture::new();
    let from = fx.elsewhere("recording");
    copy_private(&synthetic(), &from);
    // A long component keeps an accidental recursive copy bounded.
    let to = from.join("n".repeat(180));
    let result = replay::redact_recording(&from, &to, &Key::new("nested-path"));
    let error = result.expect_err("a nested output must be refused");
    assert!(error.contains("must be outside source"), "{error}");
    assert!(!to.exists(), "refusal must happen before creating output");
}
