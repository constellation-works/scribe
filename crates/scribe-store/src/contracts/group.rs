//! The group shapes the store hands its callers.

/// The group metadata and summary carried by a project brief.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LastGroup {
    /// Stable group id.
    pub id: String,
    /// Group title.
    pub title: String,
    /// Closing time, if closed.
    pub closed: Option<String>,
    /// Summary produced at close, if still retained.
    pub summary: Option<String>,
}

/// The number of a group id `g-<n>`, when it is one exactly.
pub fn number(id: &str) -> Option<i64> {
    let n = id.strip_prefix("g-")?.parse::<i64>().ok()?;
    (n > 0 && format!("g-{n}") == id).then_some(n)
}
