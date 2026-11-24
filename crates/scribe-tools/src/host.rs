//! Facts about this host that are neither files nor environment: its name.

/// This host's node name, when it has a non-empty UTF-8 one.
#[cfg(unix)]
#[must_use]
pub fn name() -> Option<String> {
    let uname = rustix::system::uname();
    uname
        .nodename()
        .to_str()
        .ok()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
}
