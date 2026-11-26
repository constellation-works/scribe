//! Project attribution from path mentions in transcript bytes.

use crate::record::Project;
use std::collections::BTreeMap;

/// How often each configured project's paths occur in `bytes`, summed per
/// project. Counts are non-overlapping within one path. Every project is
/// present, including those with a count of zero. Duplicate paths of one
/// project are counted once.
#[must_use]
pub fn mention_counts(
    bytes: &[u8],
    projects: &BTreeMap<Project, Vec<String>>,
) -> BTreeMap<Project, u32> {
    let text = String::from_utf8_lossy(bytes);
    projects
        .iter()
        .map(|(project, paths)| {
            let mut seen = Vec::new();
            let mut count = 0u32;
            for path in paths {
                if path.is_empty() || seen.iter().any(|seen: &String| seen == path) {
                    continue;
                }
                seen.push(path.clone());
                count = count.saturating_add(count_non_overlapping(&text, path));
            }
            (project.clone(), count)
        })
        .collect()
}

/// The project with the highest mention count, or `cwd` when every count is
/// zero. A tie takes the lexicographically smaller slug.
#[must_use]
pub fn attribute(counts: &BTreeMap<Project, u32>, cwd: &Project) -> Project {
    let mut best: Option<(&Project, u32)> = None;
    for (project, count) in counts {
        if *count == 0 {
            continue;
        }
        match best {
            Some((_, n)) if *count > n => best = Some((project, *count)),
            None => best = Some((project, *count)),
            Some(_) => {}
        }
    }
    best.map_or_else(|| cwd.clone(), |(project, _)| project.clone())
}

fn count_non_overlapping(haystack: &str, needle: &str) -> u32 {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0u32;
    let mut rest = haystack;
    while let Some(at) = rest.find(needle) {
        count = count.saturating_add(1);
        rest = &rest[at + needle.len()..];
    }
    count
}
