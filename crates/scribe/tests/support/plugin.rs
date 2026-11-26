use crate::support::command::Exit::Capture;
use crate::support::command::{Expectation, Input};
use crate::support::{Fixture, text};
use serde_json::{Value, json};
use std::process::Output;

const CLAUDE: &str = "#!/bin/sh\n\
    echo call >> \"$HOME/claude-calls\"\n\
    cat > /dev/null\n\
    printf '%s\\n' '{\"type\":\"result\",\"is_error\":false,\"result\":\"\",\"structured_output\":{\"status\":\"answered\",\"answer\":\"Squash.\",\"claims\":[{\"text\":\"squash\",\"quote\":\"squash-merge agent PRs;\",\"cite\":\"preferences-2\"}],\"conflicts\":[]}}'\n";

impl Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input
            .into()
            .run_with(self, &["plugin", "call"], expect, |fx, cmd| {
                // The envelope supplies the root, and the seeded redaction config needs this variable.
                cmd.env_remove("SCRIBE_ROOT")
                    .env("PATH", fx.program_path())
                    .env("GITHUB_TOKEN", "ghp_testtoken1234567890");
            })
    }
}

pub(super) struct Setup {
    pub(super) fx: Fixture,
}

impl Setup {
    pub(super) fn new() -> Self {
        let fx = Fixture::seeded();
        fx.fake_program("claude", CLAUDE);
        Self { fx }
    }

    /// Run `plugin call` with `envelope` on stdin. `SCRIBE_ROOT` is removed:
    /// the tool's store comes from the envelope alone.
    #[track_caller]
    fn call(&self, envelope: &[u8]) -> Output {
        self.fx.check(&[], Capture.stdin(envelope))
    }

    /// The reply envelope for `tool` with `input` and `config`; exit 0 and
    /// one line, whatever the outcome.
    #[track_caller]
    pub(super) fn reply(&self, tool: &str, input: Value, config: Value) -> Value {
        let envelope = json!({
            "schema_version": 1, "tool": tool, "input": input,
            "context": {"workspace_root": "/nonexistent", "agent": "claude", "model": null,
                        "config": config, "task_id": null, "job_run_id": null}
        });
        self.reply_raw(envelope.to_string().as_bytes())
    }

    #[track_caller]
    pub(super) fn reply_raw(&self, envelope: &[u8]) -> Value {
        let out = self.call(envelope);
        assert_eq!(out.status.code(), Some(0), "{}", text(&out.stderr));
        let stdout = text(&out.stdout);
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        serde_json::from_str(&stdout).unwrap()
    }

    pub(super) fn root_config(&self) -> Value {
        json!({"root": self.fx.root().to_str().unwrap(), "task_workspaces": []})
    }
}

pub(super) fn error_code(reply: &Value) -> &str {
    assert_eq!(reply["ok"], false, "{reply}");
    reply["error"]["code"].as_str().unwrap()
}
