//! Drive the actual prepare/apply CLI or plugin tools with fixed activity
//! results. The fixture joins per-window reports for assertions about a run.

use super::{ChildGuard, Fixture};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Output, Stdio};

#[derive(Debug)]
pub struct ActivityFailure {
    pub code: &'static str,
    pub reason: String,
}

impl ActivityFailure {
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            code: "model_failed",
            reason: reason.into(),
        }
    }
}

pub fn call(command: &mut Command, input: &Value) -> Output {
    let child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = ChildGuard::new(child);
    guard
        .child()
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    guard.finish()
}

pub fn cli_step(
    fixture: &Fixture,
    env: &[(&str, &str)],
    step: &str,
    input: Value,
) -> Result<Value, Output> {
    if step == "fail" {
        return plugin_step(
            fixture,
            env,
            step,
            input,
            &json!({
                "root": fixture.root(), "transcripts": fixture.home().join(".claude/projects")
            }),
        );
    }
    let output = call(
        fixture
            .cmd(&["session", &format!("pass-{step}"), "--json"])
            .envs(env.iter().copied()),
        &input,
    );
    if output.status.success() {
        Ok(serde_json::from_slice(&output.stdout).unwrap())
    } else {
        Err(output)
    }
}

pub fn plugin_step(
    fixture: &Fixture,
    env: &[(&str, &str)],
    step: &str,
    input: Value,
    config: &Value,
) -> Result<Value, Output> {
    plugin_tool(fixture, env, &format!("session-pass-{step}"), input, config)
}

/// Call the plugin tool `scribe.<tool>` through `scribe plugin call`.
pub fn plugin_tool(
    fixture: &Fixture,
    env: &[(&str, &str)],
    tool: &str,
    input: Value,
    config: &Value,
) -> Result<Value, Output> {
    let mut command = fixture.cmd(&["plugin", "call"]);
    command.env_remove("SCRIBE_ROOT").envs(env.iter().copied());
    let reply = call(
        &mut command,
        &json!({
            "schema_version": 1, "tool": format!("scribe.{tool}"),
            "input": input, "context": {"config": config},
        }),
    );
    assert!(reply.status.success(), "{}", super::text(&reply.stderr));
    let reply_json: Value = serde_json::from_slice(&reply.stdout).unwrap();
    if reply_json["ok"] == true {
        Ok(reply_json["output"].clone())
    } else {
        Err(reply)
    }
}

pub fn run(
    mut step: impl FnMut(&str, Value) -> Result<Value, Output>,
    mut selection: Value,
    mut response: impl FnMut(&Value, bool) -> Result<Value, ActivityFailure>,
) -> Result<Value, Output> {
    let mut passes = std::collections::BTreeMap::new();
    let mut finished = None;
    for _ in 0..25 {
        let prepared = step("prepare", selection.clone())?;
        let cursor = &prepared["cursor"];
        let remaining = |cursor: &Value| {
            cursor["remaining"].as_u64().unwrap()
                + (cursor["sessions"].as_array().unwrap().len() as u64)
                    .saturating_sub(cursor["next"].as_u64().unwrap())
        };
        finished = Some(remaining(cursor));
        if prepared["done"] == true {
            break;
        }
        if prepared["needs_model"] == false {
            for settled in prepared["settled"].as_array().unwrap() {
                add_pass(&mut passes, settled.clone());
            }
            selection["progress"] = json!([{"cursor": cursor}]);
            continue;
        }
        let mut input = json!({"basis": prepared["basis"], "cursor": cursor});
        let first = response(&prepared["window_request"], false);
        let mut correction = None;
        let applied = match first {
            Ok(answer) => {
                input["answer"] = answer;
                let applied = step("apply", input.clone())?;
                let header = applied["status"] == "retry_header";
                if header || applied["status"] == "retry_window" {
                    let key = if header {
                        "header_request"
                    } else {
                        "window_request"
                    };
                    correction = applied["correction"].as_str().map(str::to_owned);
                    match response(&applied[key], header) {
                        Ok(result) => {
                            input[if header { "header" } else { "retry_answer" }] = result;
                            input["previous_status"] = applied["status"].clone();
                            input["correction"] = applied["correction"].clone();
                            input["repair"] = json!(true);
                            Ok(step("apply", input)?)
                        }
                        Err(err) => Err(err),
                    }
                } else {
                    Ok(applied)
                }
            }
            Err(err) => Err(err),
        };
        match applied {
            Ok(applied) => {
                add_pass(&mut passes, applied["pass"].clone());
                selection["progress"] = json!([{"cursor": applied["cursor"]}]);
                finished = Some(remaining(&applied["cursor"]));
            }
            Err(failure) => {
                let reason = correction.map_or(failure.reason.clone(), |why| {
                    format!("{why}; retry: {}", failure.reason)
                });
                let failed = step(
                    "fail",
                    json!({
                        "basis": prepared["basis"], "error_code": failure.code, "reason": reason,
                    }),
                )?;
                add_pass(&mut passes, failed["pass"].clone());
                // As the job does: apply without an answer moves the run on
                // to the next session.
                let moved = step(
                    "apply",
                    json!({"basis": prepared["basis"], "cursor": cursor}),
                )?;
                assert_eq!(moved["status"], "failed", "{moved}");
                assert_eq!(moved["pass"], Value::Null, "{moved}");
                let next = moved["cursor"].clone();
                finished = Some(remaining(&next));
                selection["progress"] = json!([{"cursor": next}]);
            }
        }
    }
    Ok(json!({"passes": passes.into_values().collect::<Vec<_>>(), "remaining": finished.unwrap()}))
}

fn add_pass(passes: &mut std::collections::BTreeMap<String, Value>, mut pass: Value) {
    let Some(session) = pass["session"].as_str().map(str::to_owned) else {
        return;
    };
    if let Some(previous) = passes.get(&session) {
        for field in [
            "windows",
            "segments_sealed",
            "dropped_terms",
            "memory_written",
            "memory_pending",
        ] {
            pass[field] = json!(pass[field].as_u64().unwrap() + previous[field].as_u64().unwrap());
        }
        let mut warnings = previous["warnings"].as_array().unwrap().clone();
        warnings.extend(pass["warnings"].as_array().unwrap().iter().cloned());
        pass["warnings"] = json!(warnings);
        for (code, count) in previous["memory_refused"].as_object().unwrap() {
            pass["memory_refused"][code] =
                json!(pass["memory_refused"][code].as_u64().unwrap_or(0) + count.as_u64().unwrap());
        }
    }
    passes.insert(session, pass);
}

pub fn fill_window(answer: &mut Value, request: &Value, force_new: bool) {
    if answer["new"][0]["starts_at"] == "__window_first__" {
        if !force_new && request["previous"]["open"].is_object() {
            let mut segment = answer["new"].as_array_mut().unwrap().remove(0);
            segment.as_object_mut().unwrap().remove("starts_at");
            answer["open"] = segment;
        } else {
            answer["new"][0]["starts_at"] = request["messages"][0]["uuid"].clone();
        }
    }
}

pub fn header(answer: &Value) -> Value {
    json!({"title": answer["title"], "summary": answer["summary"], "goals": answer["goals"], "next": answer["next"]})
}

/// The plugin config of a fixture whose transcripts are Claude Code's.
pub fn claude_config(fixture: &Fixture) -> Value {
    json!({"root": fixture.root(), "transcripts": fixture.home().join(".claude/projects")})
}

/// Run the `scribe-session-rebuild` job's steps as the job does: prepare,
/// apply, at most one targeted retry, finish, until finish is done.
/// `response` answers each activity (`true` for the header compression).
///
/// The result is the run's report (`{passes, remaining}`) as the
/// in-process rebuild printed it, with exit status 0; when a step fails the
/// run, the report names the failure with the windows written so far and
/// the step's error envelope is on stderr, with exit status 1.
pub fn rebuild(
    mut step: impl FnMut(&str, Value) -> Result<Value, Output>,
    input: &Value,
    mut response: impl FnMut(&Value, bool) -> Result<Value, ActivityFailure>,
) -> Output {
    let mut progress = json!([]);
    let mut windows = 0;
    let failed = |reply: Output, windows: u64| -> Output {
        let error = serde_json::from_slice::<Value>(&reply.stdout)
            .map(|reply| reply["error"].clone())
            .unwrap_or(Value::Null);
        let report = json!({"passes": [{
            "session": input["session"], "result": "failed", "error_code": error["code"],
            "error": error["message"], "windows": windows,
        }], "remaining": 0});
        Output {
            status: std::os::unix::process::ExitStatusExt::from_raw(1 << 8),
            stdout: serde_json::to_vec(&report).unwrap(),
            stderr: serde_json::to_vec(&error).unwrap(),
        }
    };
    for _ in 0..100 {
        let mut prepare_input = input.clone();
        prepare_input["progress"] = progress.clone();
        let prepared = match step("rebuild-prepare", prepare_input) {
            Ok(prepared) => prepared,
            Err(reply) => return failed(reply, windows),
        };
        let basis = prepared["basis"].clone();
        let cursor = prepared["cursor"].clone();
        let mut first = json!({"basis": basis, "cursor": cursor});
        if prepared["needs_model"] == true {
            match response(&prepared["window_request"], false) {
                Ok(answer) => first["answer"] = answer,
                Err(failure) => {
                    first["answer"] = json!([null, null]);
                    first["errors"] = json!([failure.reason, null]);
                }
            }
        }
        let applied = match step("rebuild-apply", first.clone()) {
            Ok(applied) => applied,
            Err(reply) => return failed(reply, windows),
        };
        let mut last = first;
        last["cursor"] = applied["cursor"].clone();
        last["repair"] = json!(true);
        last["previous_status"] = applied["status"].clone();
        last["correction"] = applied["correction"].clone();
        let header = applied["status"] == "retry_header";
        if header || applied["status"] == "retry_window" {
            let request = &applied[if header {
                "header_request"
            } else {
                "window_request"
            }];
            let (answer, errors) = if header {
                ("header", "header_errors")
            } else {
                ("retry_answer", "retry_errors")
            };
            match response(request, header) {
                Ok(result) => last[answer] = result,
                Err(failure) => {
                    last[answer] = json!([null, null]);
                    last[errors] = json!([failure.reason, null]);
                }
            }
        }
        let carried = match step("rebuild-apply", last) {
            Ok(carried) => carried,
            Err(reply) => return failed(reply, windows),
        };
        windows = carried["cursor"]["windows"].as_u64().unwrap();
        progress = json!([{"cursor": carried["cursor"]}]);
        let finished = match step(
            "rebuild-finish",
            json!({"basis": basis, "progress": progress}),
        ) {
            Ok(finished) => finished,
            Err(reply) => return failed(reply, windows),
        };
        if finished["done"] == true {
            let warnings: Vec<String> = finished["pass"]["warnings"]
                .as_array()
                .unwrap()
                .iter()
                .map(|w| w.as_str().unwrap().to_owned())
                .collect();
            return Output {
                status: std::os::unix::process::ExitStatusExt::from_raw(0),
                stdout: serde_json::to_vec(&json!({"passes": [finished["pass"]], "remaining": 0}))
                    .unwrap(),
                stderr: warnings.join("\n").into_bytes(),
            };
        }
    }
    panic!("the rebuild did not finish in 100 iterations");
}
