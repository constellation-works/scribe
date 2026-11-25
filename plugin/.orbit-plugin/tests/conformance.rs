//! The plugin manifest's surface contract (2_design.md §1), compiled into
//! `crates/scribe/tests/plugin.rs` and run by `cargo test`: no tool is
//! advertised to agents over MCP, and the routine tools are mutating and
//! not agent-callable. `orbit plugin test` runs the YAML goldens beside
//! this file.

use std::path::PathBuf;
use yaml_serde::Value;

fn manifest() -> Value {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../plugin/.orbit-plugin/plugin.yaml");
    let text = std::fs::read_to_string(&path).expect("read plugin.yaml");
    yaml_serde::from_str(&text).expect("plugin.yaml is YAML")
}

fn tools(manifest: &Value) -> Vec<&Value> {
    manifest["spec"]["tools"]
        .as_sequence()
        .expect("spec.tools is a list")
        .iter()
        .collect()
}

fn field<'a>(tool: &'a Value, key: &str) -> Option<&'a str> {
    tool.get(key).and_then(Value::as_str)
}

#[test]
fn no_tool_has_an_mcp_scope_other_than_none() {
    let manifest = manifest();
    let advertised: Vec<&str> = tools(&manifest)
        .into_iter()
        // Orbit defaults an absent mcp_scope to `workspace`.
        .filter(|t| field(t, "mcp_scope").unwrap_or("workspace") != "none")
        .map(|t| field(t, "name").expect("a tool has a name"))
        .collect();
    assert_eq!(advertised, Vec::<&str>::new());
}

#[test]
fn the_routine_tools_are_not_agent_callable() {
    let manifest = manifest();
    assert_eq!(manifest["metadata"]["name"].as_str(), Some("scribe"));
    for tool in tools(&manifest) {
        let name = field(tool, "name").expect("a tool has a name");
        let kind = if name == "group-close-prepare" {
            "read_only"
        } else {
            "mutating"
        };
        assert_eq!(field(tool, "execution_kind"), Some(kind), "{name}");
        assert_eq!(
            tool.get("agent_callable").and_then(Value::as_bool),
            Some(false),
            "{name}"
        );
    }
}

fn yaml(relative: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/.orbit-plugin")
        .join(relative);
    let text = std::fs::read_to_string(&path).expect("read a definition");
    yaml_serde::from_str(&text).expect("a definition is YAML")
}

fn listed<'a>(manifest: &'a Value, kind: &str) -> Vec<&'a str> {
    manifest["spec"]["definitions"][kind]
        .as_sequence()
        .expect("spec.definitions lists it")
        .iter()
        .map(|v| v.as_str().expect("a path"))
        .collect()
}

#[test]
fn the_routines_ship_disabled_and_target_their_jobs() {
    let manifest = manifest();
    let jobs: Vec<Value> = listed(&manifest, "jobs").into_iter().map(yaml).collect();
    let mut single_step_tools = Vec::new();
    let mut saw_memory_refresh = false;
    for routine in listed(&manifest, "routines").into_iter().map(yaml) {
        let name = routine["name"].as_str().expect("a routine has a name");
        assert_eq!(routine["enabled"].as_bool(), Some(false), "{name}");
        let target = routine["target"].as_str().expect("a target");
        let job_name = target.strip_prefix("job:").expect("a job target");
        let job = jobs
            .iter()
            .find(|j| j["metadata"]["name"].as_str() == Some(job_name))
            .unwrap_or_else(|| panic!("{name}: no job {job_name}"));
        let steps = job["spec"]["steps"].as_sequence().expect("steps");
        if job_name == "scribe-memory-refresh" {
            saw_memory_refresh = true;
            assert_eq!(steps.len(), 5, "{name}");
            assert_eq!(
                steps[0]["spec"]["config"]["tool"].as_str(),
                Some("scribe.memory-refresh-prepare")
            );
            assert_eq!(
                steps[1]["target"].as_str(),
                Some("activity:scribe_profile_refresh")
            );
            assert_eq!(
                steps[2]["spec"]["config"]["tool"].as_str(),
                Some("scribe.memory-refresh-apply")
            );
            assert_eq!(
                steps[3]["target"].as_str(),
                Some("activity:scribe_profile_refresh")
            );
            assert_eq!(
                steps[4]["spec"]["config"]["tool"].as_str(),
                Some("scribe.memory-refresh-apply")
            );
            continue;
        }
        if job_name == "scribe-session-pass" {
            assert_eq!(routine["trigger"]["cron"].as_str(), Some("*/5 * * * *"));
            assert_eq!(routine["policy"]["overlap"].as_str(), Some("forbid"));
            assert_eq!(job["spec"]["max_active_runs"].as_u64(), Some(1));
            let iteration = &steps[1]["loop"];
            assert_eq!(iteration["max_iterations"].as_u64(), Some(25));
            // Every step that runs work has a timeout (a fan-out step's is on
            // its worker), and no failure fallback ends the run for the
            // sessions after it.
            let mut timed = Vec::new();
            let mut pending: Vec<&Value> = steps.iter().collect();
            while let Some(step) = pending.pop() {
                if let Some(body) = step["loop"]["steps"].as_sequence() {
                    pending.extend(body);
                } else if !step["fan_out"].is_null() {
                    pending.push(&step["fan_out"]["worker"]);
                } else {
                    let id = step["id"].as_str().unwrap();
                    assert!(
                        step["timeout_seconds"].as_u64().is_some_and(|t| t > 0),
                        "{id} has no timeout_seconds"
                    );
                    assert!(
                        step["default_input"]["input"]["abort"].is_null(),
                        "{id} aborts the run"
                    );
                    timed.push(id);
                }
            }
            assert!(timed.contains(&"fail_distill") && timed.contains(&"distill_window"));
            let distill = yaml("activities/session_distill.yaml");
            let header = yaml("activities/session_header_compress.yaml");
            for (activity, schema) in [
                (distill, scribe_core::models::session_schema_with_memory()),
                (
                    header,
                    scribe_core::models::SESSION_HEADER_SCHEMA.to_owned(),
                ),
            ] {
                let spec = &activity["spec"];
                assert_eq!(spec["fsProfile"].as_str(), Some("reviewer"));
                assert_eq!(spec["require_response_envelope"].as_bool(), Some(true));
                assert_eq!(spec["tools"].as_sequence().unwrap().len(), 0);
                assert_eq!(spec["on_denial"].as_str(), Some("terminate"));
                let actual = serde_json::to_value(&spec["output_schema_json"]).unwrap();
                assert_eq!(
                    actual,
                    serde_json::from_str::<serde_json::Value>(&schema).unwrap()
                );
            }
            continue;
        }
        assert_eq!(steps.len(), 1, "{name}");
        let config = &steps[0]["spec"]["config"];
        assert_eq!(
            steps[0]["spec"]["action"].as_str(),
            Some("plugin.tool_call")
        );
        single_step_tools.push(config["tool"].as_str().expect("a tool").to_owned());
    }
    assert!(saw_memory_refresh, "scribe-memory-refresh routine exists");
    single_step_tools.sort();
    assert_eq!(single_step_tools, ["scribe.consistency-scan"]);
}

#[test]
fn no_tool_settles_a_proposal_or_changes_an_entry_by_id() {
    // Settling is Daniel's, typed in a prompt (#scribe approve|reject|
    // retire|forget): the tools are the routine and workflow tools only,
    // and the no-arg tools take no input.
    let manifest = manifest();
    let names: Vec<&str> = tools(&manifest)
        .into_iter()
        .map(|t| field(t, "name").expect("a tool has a name"))
        .collect();
    assert_eq!(
        names,
        [
            "session-pass-prepare",
            "session-pass-apply",
            "session-pass-fail",
            "consistency-scan",
            "memory-refresh-prepare",
            "memory-refresh-apply",
            "group-close-prepare",
            "group-close-apply"
        ]
    );
    let empty = yaml("schemas/empty.input.json");
    assert_eq!(empty["additionalProperties"].as_bool(), Some(false));
    assert!(empty.get("properties").is_none());
}

#[test]
fn codex_package_keeps_the_tool_boundary_and_only_adds_narrow_runtime_writes() {
    let base = manifest();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/orbit-codex/.orbit-plugin/plugin.yaml");
    let codex: Value = yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(codex["metadata"], base["metadata"]);
    assert_eq!(codex["spec"]["tools"], base["spec"]["tools"]);
    assert_eq!(codex["spec"]["definitions"], base["spec"]["definitions"]);
    assert_eq!(codex["spec"]["backend"], base["spec"]["backend"]);
    let fs = &codex["spec"]["permissions"]["fs"];
    let writes: Vec<_> = fs["write"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(
        writes,
        [
            "{{config.root}}",
            "{{plugin_state}}",
            "{{config.codex_runtime_tmp}}"
        ]
    );
    let files: Vec<_> = fs["write_files"]
        .as_sequence()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(files, ["{{config.codex_installation_id}}"]);
    assert!(
        base["spec"]["permissions"]["fs"]
            .get("write_files")
            .is_none()
    );
}

/// Each `{{config.<key>}}` a manifest's `permissions.fs` lists names, in order.
fn fs_config_keys(manifest: &Value) -> Vec<String> {
    let fs = &manifest["spec"]["permissions"]["fs"];
    let mut keys = Vec::new();
    for list in ["read", "write", "write_files"] {
        for entry in fs[list].as_sequence().into_iter().flatten() {
            let mut rest = entry.as_str().expect("an fs entry is a string");
            while let Some(start) = rest.find("{{config.") {
                let after = &rest[start + "{{config.".len()..];
                let end = after.find("}}").expect("a template reference is closed");
                keys.push(after[..end].to_string());
                rest = &after[end + 2..];
            }
        }
    }
    keys
}

fn declared_defaults(manifest: &Value) -> Vec<&str> {
    manifest["spec"]["config"]["defaults"]
        .as_mapping()
        .expect("spec.config.defaults is a mapping")
        .keys()
        .map(|key| key.as_str().expect("a default key is a string"))
        .collect()
}

// Orbit renders every `{{config.<key>}}` in `permissions.fs` when it loads
// the plugin, and refuses it when the key has neither a configured value nor
// a manifest default. The default package must load on a host that sets
// nothing (`orbit plugin upgrade scribe plugin/.orbit-plugin` on a fresh or
// Claude-only host): every key it renders carries a default (ORB-26579).
#[test]
fn the_default_package_declares_a_default_for_every_config_key_its_grants_render() {
    let base = manifest();
    let defaults = declared_defaults(&base);
    let keys = fs_config_keys(&base);
    assert_eq!(keys, ["root", "transcripts", "codex_home", "root"]);
    for key in &keys {
        assert!(defaults.contains(&key.as_str()), "no default for {key}");
    }
}

// The Codex package's three provider paths are explicit consent: a default
// would grant reads or writes the operator never named, and an exact-file
// write cannot name a harmless placeholder. So they carry no default, and
// Orbit refuses the package until all three are configured (its README says
// how to recover). Every other key it renders keeps the default package's.
#[test]
fn the_codex_package_leaves_only_its_three_provider_paths_without_a_default() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/orbit-codex/.orbit-plugin/plugin.yaml");
    let codex: Value = yaml_serde::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let defaults = declared_defaults(&codex);
    let undefaulted: Vec<String> = fs_config_keys(&codex)
        .into_iter()
        .filter(|key| !defaults.contains(&key.as_str()))
        .collect();
    assert_eq!(
        undefaulted,
        ["codex_home", "codex_runtime_tmp", "codex_installation_id"]
    );
}
