//! The plugin manifest's surface contract (2_design.md §1), compiled into
//! `crates/scribe/tests/plugin.rs` and run by `cargo test`: `scribe.ask` is
//! the only tool agents see over MCP, and the routine tools are mutating
//! and not agent-callable. `orbit plugin test` runs the YAML goldens beside
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
fn ask_is_the_only_tool_with_an_mcp_scope_other_than_none() {
    let manifest = manifest();
    let advertised: Vec<&str> = tools(&manifest)
        .into_iter()
        // Orbit defaults an absent mcp_scope to `workspace`.
        .filter(|t| field(t, "mcp_scope").unwrap_or("workspace") != "none")
        .map(|t| field(t, "name").expect("a tool has a name"))
        .collect();
    assert_eq!(advertised, vec!["ask"]);
}

#[test]
fn ask_is_read_only_and_global_and_the_routine_tools_are_not_agent_callable() {
    let manifest = manifest();
    assert_eq!(manifest["metadata"]["name"].as_str(), Some("scribe"));
    for tool in tools(&manifest) {
        let name = field(tool, "name").expect("a tool has a name");
        if name == "ask" {
            assert_eq!(field(tool, "execution_kind"), Some("read_only"));
            assert_eq!(field(tool, "mcp_scope"), Some("global"));
        } else {
            assert_eq!(field(tool, "execution_kind"), Some("mutating"), "{name}");
            assert_eq!(
                tool.get("agent_callable").and_then(Value::as_bool),
                Some(false),
                "{name}"
            );
        }
    }
}
