use super::*;

/// A Claude Code layout under the fixture's home, a fake `claude` on PATH
/// and helpers to run the binary with them.
pub(crate) struct Setup {
    pub(crate) fx: Fixture,
    pub(crate) project: PathBuf,
    pub(crate) cwd: PathBuf,
    pub(crate) path: OsString,
    pub(crate) responses: RefCell<Responses>,
    pub(crate) before_answer: RefCell<Option<Box<dyn FnOnce()>>>,
    /// How many answers pass before `before_answer` runs.
    pub(crate) before_answer_after: std::cell::Cell<usize>,
}

pub(crate) enum Responses {
    Fixed(Value, bool),
    Sequence(Vec<Option<Value>>),
    Failure(&'static str, String),
}

pub(crate) fn line(kind: &str, uuid: &str, text: &str) -> Value {
    json!({"type": kind, "uuid": uuid, "timestamp": "2026-11-03T06:00:00Z",
           "cwd": "/nonexistent/work/scribe",
           "message": {"role": kind, "content": text}})
}

impl Setup {
    pub(crate) fn new() -> Self {
        Self::in_fixture(Fixture::new())
    }

    pub(crate) fn in_fixture(fx: Fixture) -> Self {
        let project = fx.home().join(".claude/projects/-work-scribe");
        std::fs::create_dir_all(&project).unwrap();
        let cwd = fx.elsewhere("work/scribe");
        std::fs::create_dir_all(&cwd).unwrap();
        let bin = fx.elsewhere("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let mut path = bin.into_os_string();
        path.push(":/usr/bin:/bin");
        let setup = Self {
            fx,
            project,
            cwd,
            path,
            responses: RefCell::new(Responses::Fixed(answer(), false)),
            before_answer: RefCell::new(None),
            before_answer_after: std::cell::Cell::new(0),
        };
        setup.answer(&answer());
        setup.write(ON, &[line("user", "u-1", &format!("Crews. {DEFINED}."))]);
        setup.append(
            ON,
            &[line(
                "assistant",
                "a-1",
                "Level 4 crews merge their own pull requests after green CI.",
            )],
        );
        setup.write(NEVER, &[line("user", "n-1", NEVER_SAID)]);
        setup
    }

    pub(crate) fn transcript(&self, session: &str) -> PathBuf {
        self.project.join(format!("{session}.jsonl"))
    }

    pub(crate) fn write(&self, session: &str, lines: &[Value]) {
        let text: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let path = self.transcript(session);
        std::fs::write(&path, text).unwrap();
        make_private(&path);
    }

    pub(crate) fn append(&self, session: &str, lines: &[Value]) {
        let mut text = std::fs::read_to_string(self.transcript(session)).unwrap_or_default();
        text.extend(lines.iter().map(|l| format!("{l}\n")));
        let path = self.transcript(session);
        std::fs::write(&path, text).unwrap();
        make_private(&path);
    }

    /// The fake `claude`: saves its stdin (the prompt) and answers `body`
    /// as `claude -p --output-format json` does.
    pub(crate) fn answer(&self, body: &Value) {
        self.responses
            .replace(Responses::Fixed(body.clone(), false));
        let script = format!(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\n{}",
            response(body)
        );
        self.fx.fake_program("claude", &script);
    }

    /// Every prompt the fake `claude` received, oldest first.
    pub(crate) fn prompts(&self) -> Vec<String> {
        (0..)
            .map(|n| self.fx.home().join(format!("claude-stdin-{n}")))
            .take_while(|p| p.exists())
            .map(|p| std::fs::read_to_string(p).unwrap())
            .collect()
    }

    /// Scripted answers across process invocations; `None` fails that call.
    pub(crate) fn answers(&self, answers: &[Option<Value>]) {
        self.responses
            .replace(Responses::Sequence(answers.to_vec()));
        let mut script = String::from(
            "#!/bin/sh\nn=$(ls \"$HOME\" | grep -c '^claude-stdin')\ncat > \"$HOME/claude-stdin-$n\"\ncase $n in\n",
        );
        for (n, answer) in answers.iter().enumerate() {
            match answer {
                Some(body) => {
                    script.push_str(&format!("{n}) {} ;;\n", response(body)));
                }
                None => script.push_str(&format!("{n}) exit 3 ;;\n")),
            }
        }
        script.push_str("*) exit 4 ;;\nesac\n");
        self.fx.fake_program("claude", &script);
    }

    /// Run the pass workflow with fixed activity responses. Aggregate its
    /// per-step output for the existing end-to-end record assertions.
    pub(crate) fn run_pass(&self, args: &[&str], env: &[(&str, &str)]) -> Output {
        let sessions: Vec<_> = args
            .windows(2)
            .filter(|pair| pair[0] == "--session")
            .map(|pair| pair[1])
            .collect();
        let result = pass::run(
            |step, input| pass::cli_step(&self.fx, env, step, input),
            json!({"sessions": sessions, "resolve": true}),
            |request, header| self.fixed_response(request, header),
        );
        Self::reported(result, args)
    }

    /// Run the `scribe-session-catalog` job's plugin steps with fixed
    /// activity responses, for `session catalog <id> --author <a>`: begin
    /// writes or resumes the switch, then the pass steps run in catalog mode.
    pub(crate) fn run_catalog(&self, args: &[&str]) -> Output {
        let author = args
            .windows(2)
            .find(|pair| pair[0] == "--author")
            .map(|pair| pair[1])
            .unwrap();
        let config = pass::claude_config(&self.fx);
        let step = |tool: &str, input| pass::plugin_tool(&self.fx, &[], tool, input, &config);
        let begun = match step(
            "session-catalog-begin",
            json!({"session": args[2], "author": author}),
        ) {
            Ok(begun) => begun,
            Err(output) => return Self::reported(Err(output), args),
        };
        let result = pass::run(
            |name, input| step(&format!("session-pass-{name}"), input),
            json!({"sessions": begun["sessions"], "resolve": false, "catalog": begun["catalog"]}),
            |request, header| self.fixed_response(request, header),
        );
        Self::reported(result, args)
    }

    /// A run's result as the in-process command reported it: the joined
    /// payloads, and exit status 1 with a diagnostic when a pass failed.
    pub(crate) fn reported(result: Result<Value, Output>, args: &[&str]) -> Output {
        let result = match result {
            Ok(result) => result,
            Err(mut output) => {
                if output.status.success() {
                    // Plugin tool failures are encoded in the envelope.
                    let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
                    output.stderr = serde_json::to_vec(&reply["error"]).unwrap();
                    output.stdout.clear();
                    output.status = std::process::ExitStatus::from_raw(1 << 8);
                }
                return output;
            }
        };
        let mut warnings = Vec::new();
        let mut failures = Vec::new();
        for payload in result["passes"].as_array().unwrap() {
            warnings.extend(
                payload["warnings"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| w.as_str().unwrap().to_owned()),
            );
            if let Some(code) = payload["error_code"].as_str() {
                failures.push(format!(
                    "{} {code}: {}",
                    payload["session"].as_str().unwrap(),
                    payload["error"].as_str().unwrap()
                ));
            }
        }
        if !failures.is_empty() {
            let diagnostic = format!(
                "{} session pass(es) failed: {}",
                failures.len(),
                failures.join(", ")
            );
            warnings.push(if args.contains(&"--json") {
                json!({"code": "session_pass_failed", "error": diagnostic}).to_string()
            } else {
                format!("error: {diagnostic}")
            });
        }
        Output {
            status: std::process::ExitStatus::from_raw(i32::from(!failures.is_empty()) << 8),
            stdout: serde_json::to_vec(&result).unwrap(),
            stderr: warnings
                .join(
                    "
",
                )
                .into_bytes(),
        }
    }

    pub(crate) fn fixed_response(
        &self,
        request: &Value,
        header: bool,
    ) -> Result<Value, ActivityFailure> {
        let index = self.prompts().len();
        let prompt = if header {
            serde_json::from_value::<scribe_core::models::SessionHeaderRequest>(request.clone())
                .unwrap()
                .prompt()
        } else {
            serde_json::from_value::<scribe_core::models::SessionWindowInput>(request.clone())
                .unwrap()
                .prompt()
        };
        std::fs::write(self.fx.home().join(format!("claude-stdin-{index}")), prompt).unwrap();
        let after = self.before_answer_after.get();
        if after > 0 {
            self.before_answer_after.set(after - 1);
        } else if let Some(change) = self.before_answer.take() {
            change();
        }
        let responses = self.responses.borrow();
        let (answer, force_new) = match &*responses {
            Responses::Fixed(answer, force_new) => (Some(answer.clone()), *force_new),
            Responses::Sequence(answers) => (answers.get(index).cloned().flatten(), false),
            Responses::Failure(code, reason) => {
                return Err(ActivityFailure {
                    code,
                    reason: reason.clone(),
                });
            }
        };
        let mut answer = with_claim_contexts(
            &answer.ok_or_else(|| ActivityFailure::failed("the model failed"))?,
        );
        if header {
            Ok(pass::header(&answer))
        } else {
            pass::fill_window(&mut answer, request, force_new);
            Ok(answer)
        }
    }

    /// Run the `scribe-session-rebuild` job's plugin steps with fixed
    /// activity responses, for `session rebuild <id> --author <a> --confirm`.
    pub(crate) fn run_rebuild(&self, args: &[&str]) -> Output {
        let author = args
            .windows(2)
            .find(|pair| pair[0] == "--author")
            .map(|pair| pair[1])
            .unwrap();
        let config = pass::claude_config(&self.fx);
        pass::rebuild(
            |step, input| {
                pass::plugin_tool(&self.fx, &[], &format!("session-{step}"), input, &config)
            },
            &json!({"session": args[2], "author": author, "confirm": args.contains(&"--confirm")}),
            |request, header| self.fixed_response(request, header),
        )
    }

    #[track_caller]
    pub(crate) fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let out = if args.starts_with(&["session", "pass"]) {
            self.run_pass(args, &[])
        } else if args.starts_with(&["session", "rebuild"]) {
            self.run_rebuild(args)
        } else {
            self.run_any(args, input)
        };
        assert_eq!(
            out.status.code(),
            Some(0),
            "{args:?}: {}",
            text(&out.stderr)
        );
        out
    }

    /// [`Setup::run`] without asserting the exit code.
    #[track_caller]
    pub(crate) fn run_any(&self, args: &[&str], input: &[u8]) -> Output {
        self.fx
            .check(with_env(args, "PATH", &self.path), Capture.stdin(input))
    }

    pub(crate) fn hook_with_env(
        &self,
        event: &str,
        payload: &Value,
        name: &str,
        value: &str,
    ) -> Output {
        self.fx.check(
            self.fx
                .cmd(&["hook", event])
                .env("PATH", &self.path)
                .env(name, value),
            Capture.stdin(payload.to_string().as_bytes()),
        )
    }

    pub(crate) fn hook(&self, event: &str, payload: &Value) -> Output {
        self.run(&["hook", event], payload.to_string().as_bytes())
    }

    pub(crate) fn payload(&self, event: &str) -> Value {
        json!({"session_id": ON,
               "transcript_path": self.transcript(ON),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": event})
    }

    /// `/scribe …` typed in the `ON` session: the hook's block reason.
    pub(crate) fn scribe(&self, prompt: &str) -> String {
        let mut payload = self.payload("UserPromptSubmit");
        payload["prompt"] = json!(prompt);
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    #[track_caller]
    pub(crate) fn pass_session(&self, session: &str) -> Output {
        self.run(&["session", "pass", "--session", session, "--json"], b"")
    }

    #[track_caller]
    pub(crate) fn entry_json(&self, id: &str) -> Value {
        self.fx.check(Cli(&["entry", "show", id, "--json"]), Json)
    }

    pub(crate) fn pass(&self) -> Value {
        let out = self.run(&["session", "pass", "--json"], b"");
        serde_json::from_slice(&out.stdout).unwrap()
    }

    pub(crate) fn control(&self) -> Value {
        support::control(&self.fx.root())["sessions"][ON].clone()
    }

    pub(crate) fn expire_retry(&self) {
        support::set_switch(&self.fx.root(), ON, "retry_after", "'2026-01-01T00:00:00Z'");
    }

    /// The switches and cursors, serialized: equal bytes mean no change.
    pub(crate) fn control_bytes(&self) -> Vec<u8> {
        support::control(&self.fx.root()).to_string().into_bytes()
    }

    /// The `ON` record as its file read (`scribe export`).
    pub(crate) fn record_text(&self) -> String {
        support::record_text(&self.fx.root(), ON).expect("the ON record")
    }

    pub(crate) fn record(&self, session: &str) -> Option<Value> {
        let out = self.fx.check(&["show", session, "--json"], Capture);
        (out.status.code() == Some(0)).then(|| serde_json::from_slice(&out.stdout).unwrap())
    }

    /// How many entries `scribe.db` holds (`0` before the db exists).
    pub(crate) fn entry_count(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.fx.root().join("scribe.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|c| c.query_row("SELECT count(*) FROM entries", [], |r| r.get(0)))
        .unwrap_or(0)
    }

    /// How many rebuild checkpoints `scribe.db` holds.
    pub(crate) fn checkpoint_count(&self) -> i64 {
        rusqlite::Connection::open_with_flags(
            self.fx.root().join("scribe.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .and_then(|c| c.query_row("SELECT count(*) FROM rebuild_checkpoints", [], |r| r.get(0)))
        .expect("the store's scribe.db")
    }

    pub(crate) fn audit_lines(&self) -> usize {
        support::audit_lines(&self.fx.root()).len()
    }

    pub(crate) fn config(&self, toml: &str) {
        let root = self.fx.root();
        if !root.exists() {
            std::fs::create_dir(&root).unwrap();
            make_private(&root);
        }
        let file = root.join("config.toml");
        std::fs::write(&file, toml).unwrap();
        make_private(&file);
    }
}

/// Generate an answer for the actual request. Compression returns only the
/// prose header. Explicit S9 window answers are passed through unchanged;
/// the generic fixture rewrites an open segment or starts a new one at the
/// first message when scribe has sealed the preceding one.
/// A session answer as the current schema returns it: a candidate a test
/// wrote without `context` gets one, [`context_of`] its `statement` (a
/// test's short form for the claim) or else of its quote, and a fixed
/// subject when it has none, so only a test that sets them (or sets one
/// empty) exercises them.
fn with_claim_contexts(body: &Value) -> Value {
    let mut body = body.clone();
    if let Some(candidates) = body.get_mut("candidates").and_then(Value::as_array_mut) {
        for candidate in candidates.iter_mut().filter_map(Value::as_object_mut) {
            let said = candidate
                .remove("statement")
                .or_else(|| candidate.get("quote").cloned())
                .and_then(|v| v.as_str().map(context_of))
                .unwrap_or_default();
            candidate.entry("context").or_insert(json!(said));
            candidate
                .entry("subject")
                .or_insert(json!("the work at hand"));
        }
    }
    body
}

pub(crate) fn response(body: &Value) -> String {
    let body = &with_claim_contexts(body);
    let envelope = |body: &Value| {
        // Single-quoted in the fake's shell script: an apostrophe closes
        // the quote, escapes itself and reopens it.
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body})
            .to_string()
            .replace('\'', "'\\''")
    };
    if body.get("new").is_some() {
        let header = json!({"title": body["title"], "summary": body["summary"],
                            "goals": body["goals"], "next": body["next"]});
        let mut script = format!(
            "if grep -q '^Compress only' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\n",
            envelope(&header)
        );
        script.push_str(&session_response(body));
        script.push_str("\nfi");
        script
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

fn session_response(body: &Value) -> String {
    let envelope = |body: &Value| {
        // Single-quoted in the fake's shell script: an apostrophe closes
        // the quote, escapes itself and reopens it.
        json!({"type":"result", "subtype":"success", "is_error":false, "result":"", "structured_output":body})
            .to_string()
            .replace('\'', "'\\''")
    };
    if body["new"][0]["starts_at"] == "__window_first__" {
        let mut open = body.clone();
        let mut segment = open["new"][0].clone();
        segment.as_object_mut().unwrap().remove("starts_at");
        open["open"] = segment;
        open["new"] = json!([]);
        format!(
            "if grep -q '^Open segment ' \"$HOME/claude-stdin-$n\"; then\nprintf '%s\\n' '{}'\nelse\nfirst=$(sed -n 's/^--- .* · \\(.*\\) ---$/\\1/p' \"$HOME/claude-stdin-$n\" | head -1)\nprintf '%s\\n' '{}' | sed \"s/__window_first__/$first/g\"\nfi",
            envelope(&open),
            envelope(body)
        )
    } else {
        format!("printf '%s\\n' '{}'", envelope(body))
    }
}

pub(crate) fn answer() -> Value {
    json!({
        "title": "Defining crew levels",
        "summary": "Daniel defined what level 4 means for crews.",
        "goals": "Name the crew levels.",
        "open": null,
        "new": [{"starts_at": "__window_first__", "heading": "Crew levels", "text": "Level 4 is defined.", "topics": [], "related": []}],
        "next": ["write it into the routing doc"],
        "topics": ["crew-routing"],
        "related": [],
        "terms": [
            {"term": "graduate to level 4", "said": DEFINED, "message": "u-1"},
            {"term": "level 4 crew", "said": "Level 4 crews merge their own pull requests",
             "message": "a-1"}
        ]
    })
}

impl Setup {
    /// `/scribe …` typed in `session`: the hook's block reason.
    pub(crate) fn scribe_in(&self, session: &str, prompt: &str) -> String {
        let payload = json!({"session_id": session,
               "transcript_path": self.transcript(session),
               "cwd": self.cwd,
               "permission_mode": "default",
               "hook_event_name": "UserPromptSubmit",
               "prompt": prompt});
        let out = self.hook("user-prompt-submit", &payload);
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["decision"], "block", "{reply}");
        reply["reason"].as_str().unwrap().to_owned()
    }

    /// The `error_code` of every audit line, oldest first.
    pub(crate) fn audit_codes(&self) -> Vec<Value> {
        support::audit_lines(&self.fx.root())
            .iter()
            .map(|l| l["error_code"].clone())
            .collect()
    }

    pub(crate) fn switch_of(&self, session: &str) -> Value {
        support::control(&self.fx.root())["sessions"][session].clone()
    }
}
