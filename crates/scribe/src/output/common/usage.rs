//! What a command line that failed to parse asked for.

use std::ffi::OsString;

/// Whether a command line that failed to parse asked for JSON, so the usage
/// error can be reported as JSON too (STD-01 R19). Parsing failed, so this
/// reads the raw arguments: `--json`, `--format json` or `--format=json`,
/// else the environment variable.
pub(crate) fn raw_args_request_json(args: &[OsString], env_format: Option<&str>) -> bool {
    let mut explicit = None;
    let mut iter = args.iter().skip(1).map(|a| a.to_string_lossy());
    while let Some(arg) = iter.next() {
        match arg.as_ref() {
            "--" => break,
            "--json" => explicit = Some(true),
            "--format" => explicit = iter.next().map(|v| v == "json"),
            other => {
                if let Some(value) = other.strip_prefix("--format=") {
                    explicit = Some(value == "json");
                }
            }
        }
    }
    explicit.unwrap_or_else(|| env_format.is_some_and(|v| v.trim().eq_ignore_ascii_case("json")))
}
