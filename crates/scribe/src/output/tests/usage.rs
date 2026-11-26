use crate::output::usage::raw_args_request_json;
use std::ffi::OsString;

fn args(list: &[&str]) -> Vec<OsString> {
    list.iter().map(OsString::from).collect()
}

#[test]
fn a_failed_parse_still_honours_a_json_request() {
    assert!(raw_args_request_json(
        &args(&["t", "entry", "--bogus", "--json"]),
        None
    ));
    assert!(raw_args_request_json(
        &args(&["t", "--format", "json", "x"]),
        None
    ));
    assert!(raw_args_request_json(
        &args(&["t", "--format=json", "x"]),
        None
    ));
    assert!(raw_args_request_json(&args(&["t", "x"]), Some("json")));
    assert!(!raw_args_request_json(
        &args(&["t", "--format", "table"]),
        Some("json")
    ));
    assert!(!raw_args_request_json(&args(&["t", "--", "--json"]), None));
}
