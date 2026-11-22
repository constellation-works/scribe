//! The issue file, `issues/<id>.md` (docs/design/scribe/2_design.md §11.4;
//! specs/store.md, "Issue files"): bytes in, an [`Issue`] out, and back.
//! No I/O (the path only names the file in an error).
//!
//! The frontmatter holds the header and each occurrence's metadata, read as
//! strictly as an entry's (every key required, an unknown one refused). The
//! body holds the text, in a fixed shape:
//!
//! ```text
//! ## Cause
//! <cause>
//! ## Fix
//! <fix>
//! ## Occurrences
//! ### <n>. <date> — <host>, <project>
//! <summary>
//! **Daniel asked:** "<request>"
//! **Error lines (agent evidence):**
//!     <error line>
//! **Cause:** <cause>
//! **Fix:** <fix>
//! **Links:** session:<id>[, <link>…]
//! ```
//!
//! One `### ` section per frontmatter occurrence, in the same order (newest
//! first), whose heading must match it. An occurrence is read from its end:
//! the links, fix and cause lines, the indented error lines under their
//! label, then the request line; everything above is the summary.

use super::format::{FORMAT, parse_frontmatter, shape, split_frontmatter};
use crate::error::{Error, Result};
use crate::issue::{Gone, GoneReason, Issue, IssueId, IssueStatus, Occurrence};
use crate::record::{Author, AuthorKind, Label, Project, Related, Topics};
use crate::session::SessionId;
use serde::{Deserialize, Serialize};
use std::num::NonZeroU32;
use std::path::Path;
use time::{Date, OffsetDateTime};

/// The record kind in the frontmatter.
const ISSUE_KIND: &str = "issue";
/// The first format an issue file can have: the kind is new at it.
const FIRST_FORMAT: u32 = 17;

const CAUSE: &str = "## Cause";
const FIX: &str = "## Fix";
const OCCURRENCES: &str = "## Occurrences";
const ASKED: &str = "**The user asked:** \"";
/// The request line's opening before format 23, still read.
const ASKED_BEFORE_23: &str = "**Daniel asked:** \"";
const ERRORS: &str = "**Error lines (agent evidence):**";
const ERROR_INDENT: &str = "    ";
const OCCURRENCE_CAUSE: &str = "**Cause:** ";
const OCCURRENCE_FIX: &str = "**Fix:** ";
const LINKS: &str = "**Links:** ";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IssueFront {
    id: String,
    title: String,
    status: String,
    signature: Vec<String>,
    projects: Vec<Project>,
    hosts: Vec<Label>,
    topics: Topics,
    related: Vec<Related>,
    author: Author,
    #[serde(deserialize_with = "crate::record::nullable")]
    updated_by: Option<Author>,
    #[serde(with = "time::serde::rfc3339")]
    created: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated: OffsetDateTime,
    occurrences: Vec<OccurrenceFront>,
    gone: Vec<GoneFront>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OccurrenceFront {
    n: NonZeroU32,
    date: String,
    host: Label,
    project: Project,
    session: SessionId,
    request_message: String,
    recurred: bool,
    #[serde(deserialize_with = "crate::record::nullable")]
    moved_from: Option<String>,
    author: Author,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoneFront {
    n: NonZeroU32,
    reason: String,
    #[serde(deserialize_with = "crate::record::nullable")]
    to: Option<String>,
}

#[derive(Serialize)]
struct FrontOut<'a> {
    format: u32,
    kind: &'static str,
    #[serde(flatten)]
    front: &'a IssueFront,
}

/// An issue file's bytes at the current format.
pub(super) fn encode_issue(path: &Path, issue: &Issue) -> Result<Vec<u8>> {
    let front = IssueFront {
        id: issue.id.to_string(),
        title: issue.title.clone(),
        status: issue.status.as_str().to_owned(),
        signature: issue.signature.clone(),
        projects: issue.projects.clone(),
        hosts: issue.hosts.clone(),
        topics: issue.topics.clone(),
        related: issue.related.clone(),
        author: issue.author.clone(),
        updated_by: issue.updated_by.clone(),
        created: issue.created,
        updated: issue.updated,
        occurrences: issue
            .occurrences
            .iter()
            .map(|o| OccurrenceFront {
                n: o.n,
                date: crate::record::DateRange::day(o.date),
                host: o.host.clone(),
                project: o.project.clone(),
                session: o.session.clone(),
                request_message: o.request_message.clone(),
                recurred: o.recurred,
                moved_from: o.moved_from.clone(),
                author: o.author.clone(),
            })
            .collect(),
        gone: issue
            .gone
            .iter()
            .map(|g| GoneFront {
                n: g.n,
                reason: g.reason.as_str().to_owned(),
                to: g.to.clone(),
            })
            .collect(),
    };
    let yaml = yaml_serde::to_string(&FrontOut {
        format: FORMAT,
        kind: ISSUE_KIND,
        front: &front,
    })
    .map_err(|e| Error::corrupt(path, e))?;
    let mut text = String::from("---\n");
    text.push_str(&yaml);
    if !yaml.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("---\n");
    text.push_str(&body(issue));
    text.push('\n');
    Ok(text.into_bytes())
}

/// The body: the header's cause and fix, then each occurrence's text.
fn body(issue: &Issue) -> String {
    let mut lines: Vec<String> = vec![CAUSE.to_owned()];
    if !issue.cause.is_empty() {
        lines.push(issue.cause.clone());
    }
    lines.push(FIX.to_owned());
    if !issue.fix.is_empty() {
        lines.push(issue.fix.clone());
    }
    lines.push(OCCURRENCES.to_owned());
    for o in &issue.occurrences {
        lines.push(format!("### {}", o.heading()));
        lines.push(o.summary.clone());
        lines.push(format!("{ASKED}{}\"", o.request));
        lines.push(ERRORS.to_owned());
        lines.extend(o.error_lines.iter().map(|l| format!("{ERROR_INDENT}{l}")));
        lines.push(format!("{OCCURRENCE_CAUSE}{}", o.cause));
        lines.push(format!("{OCCURRENCE_FIX}{}", o.fix));
        let mut links = vec![format!("session:{}", o.session)];
        links.extend(o.links.iter().map(ToString::to_string));
        lines.push(format!("{LINKS}{}", links.join(", ")));
    }
    lines.join("\n")
}

/// Parse an issue file whose stem is `expected`.
pub(super) fn decode_issue(path: &Path, bytes: &[u8], expected: &str) -> Result<Issue> {
    let (front, body) = split_frontmatter(path, bytes)?;
    let (from, value) = parse_frontmatter(path, front, ISSUE_KIND)?;
    if from < FIRST_FORMAT {
        return Err(shape(
            path,
            format!("issue records start at format {FIRST_FORMAT}; found format {from}"),
        ));
    }
    let front: IssueFront = yaml_serde::from_value(value).map_err(|e| Error::corrupt(path, e))?;
    let id: IssueId = front
        .id
        .parse()
        .map_err(|_| shape(path, format!("'{}' is not an issue id", front.id)))?;
    if id.to_string() != expected {
        return Err(shape(
            path,
            format!("id {id} does not match its file name {expected}.md"),
        ));
    }
    let status: IssueStatus = front.status.parse().map_err(|e: String| shape(path, e))?;
    let agent = |a: &Author| a.kind == AuthorKind::Agent;
    // The user changes an issue only by forgetting it or an occurrence.
    if !agent(&front.author)
        || front
            .updated_by
            .as_ref()
            .is_some_and(|a| !agent(a) && a.kind != AuthorKind::User)
    {
        return Err(shape(
            path,
            "an issue's author must be an agent, and updated_by an agent or the user",
        ));
    }
    let (cause, fix, texts) = split_body(path, body)?;
    if texts.len() != front.occurrences.len() {
        return Err(shape(
            path,
            format!(
                "{} occurrences in the frontmatter but {} in the body",
                front.occurrences.len(),
                texts.len()
            ),
        ));
    }
    let mut occurrences = Vec::with_capacity(texts.len());
    for (meta, text) in front.occurrences.into_iter().zip(texts) {
        let date = Date::parse(&meta.date, crate::record::nullable_date::FORMAT)
            .map_err(|e| Error::corrupt(path, e))?;
        if !agent(&meta.author) {
            return Err(shape(path, "an occurrence's author must be an agent"));
        }
        if meta.request_message.trim().is_empty() {
            return Err(shape(
                path,
                "an occurrence's request_message must not be empty",
            ));
        }
        let mut occurrence = Occurrence {
            n: meta.n,
            date,
            host: meta.host,
            project: meta.project,
            session: meta.session,
            request_message: meta.request_message,
            recurred: meta.recurred,
            moved_from: meta.moved_from,
            author: meta.author,
            summary: String::new(),
            request: String::new(),
            error_lines: Vec::new(),
            cause: String::new(),
            fix: String::new(),
            links: Vec::new(),
        };
        if text.heading != occurrence.heading() {
            return Err(shape(
                path,
                format!(
                    "occurrence heading '### {}' does not match its frontmatter ('### {}')",
                    text.heading,
                    occurrence.heading()
                ),
            ));
        }
        fill(path, &mut occurrence, &text.lines)?;
        occurrences.push(occurrence);
    }
    let mut numbers: Vec<u32> = occurrences.iter().map(|o| o.n.get()).collect();
    if !numbers.windows(2).all(|w| w[0] > w[1]) {
        return Err(shape(
            path,
            "occurrences must be newest (highest n) first, each n once",
        ));
    }
    let mut gone = Vec::with_capacity(front.gone.len());
    for g in front.gone {
        let reason = match g.reason.as_str() {
            "moved" => GoneReason::Moved,
            "forgotten" => GoneReason::Forgotten,
            other => return Err(shape(path, format!("'{other}' is not a gone reason"))),
        };
        if (reason == GoneReason::Moved) != g.to.is_some() {
            return Err(shape(
                path,
                "a moved occurrence names where it went; a forgotten one does not",
            ));
        }
        numbers.push(g.n.get());
        gone.push(Gone {
            n: g.n,
            reason,
            to: g.to,
        });
    }
    numbers.sort_unstable();
    if numbers.windows(2).any(|w| w[0] == w[1]) {
        return Err(shape(path, "an occurrence number appears twice"));
    }
    let forgotten = status == IssueStatus::Forgotten;
    if forgotten {
        if !(front.title.is_empty()
            && front.signature.is_empty()
            && cause.is_empty()
            && fix.is_empty()
            && occurrences.is_empty())
        {
            return Err(shape(path, "a forgotten issue keeps no text"));
        }
    } else if front.title.trim().is_empty()
        || front.signature.is_empty()
        || front.signature.len() > crate::issue::MAX_SIGNATURES
        || cause.is_empty()
        || fix.is_empty()
    {
        return Err(shape(
            path,
            "an issue needs a title, 1 to 5 signature strings, a cause and a fix",
        ));
    }
    Ok(Issue {
        id,
        title: front.title,
        status,
        signature: front.signature,
        projects: front.projects,
        hosts: front.hosts,
        topics: front.topics,
        related: front.related,
        author: front.author,
        updated_by: front.updated_by,
        created: front.created,
        updated: front.updated,
        cause,
        fix,
        occurrences,
        gone,
    })
}

/// One `### ` section of the body.
struct Section<'a> {
    heading: &'a str,
    lines: Vec<&'a str>,
}

/// The body's cause, fix and occurrence sections.
fn split_body<'a>(path: &Path, body: &'a str) -> Result<(String, String, Vec<Section<'a>>)> {
    let lines: Vec<&str> = body.split('\n').collect();
    let at = |heading: &str, from: usize| {
        lines
            .iter()
            .skip(from)
            .position(|l| *l == heading)
            .map(|i| i + from)
    };
    if lines.first() != Some(&CAUSE) {
        return Err(shape(path, "the body must open with '## Cause'"));
    }
    let fix_at = at(FIX, 1).ok_or_else(|| shape(path, "missing '## Fix'"))?;
    let occ_at =
        at(OCCURRENCES, fix_at + 1).ok_or_else(|| shape(path, "missing '## Occurrences'"))?;
    let cause = lines[1..fix_at].join("\n");
    let fix = lines[fix_at + 1..occ_at].join("\n");
    if [&cause, &fix]
        .iter()
        .any(|t| t.lines().any(|l| l.starts_with("## ")))
    {
        return Err(shape(path, "a '## ' line inside the cause or fix"));
    }
    let mut sections: Vec<Section<'a>> = Vec::new();
    for line in &lines[occ_at + 1..] {
        if let Some(heading) = line.strip_prefix("### ") {
            sections.push(Section {
                heading,
                lines: Vec::new(),
            });
        } else if line.starts_with("## ") {
            return Err(shape(path, format!("unexpected section '{line}'")));
        } else {
            match sections.last_mut() {
                Some(section) => section.lines.push(line),
                None => return Err(shape(path, "text before the first occurrence")),
            }
        }
    }
    Ok((cause, fix, sections))
}

/// Read an occurrence's text from its section lines, from the end.
fn fill(path: &Path, o: &mut Occurrence, lines: &[&str]) -> Result<()> {
    let bad = |what: &str| shape(path, format!("occurrence {}: {what}", o.n));
    let mut rest = lines;
    let mut pop = |prefix: &str| -> Option<String> {
        let (last, before) = rest.split_last()?;
        let value = last.strip_prefix(prefix)?.to_owned();
        rest = before;
        Some(value)
    };
    let links = pop(LINKS).ok_or_else(|| bad("missing its links line"))?;
    let fix = pop(OCCURRENCE_FIX).ok_or_else(|| bad("missing its fix line"))?;
    let cause = pop(OCCURRENCE_CAUSE).ok_or_else(|| bad("missing its cause line"))?;
    let mut errors = Vec::new();
    while let Some((last, before)) = rest.split_last() {
        let Some(line) = last.strip_prefix(ERROR_INDENT) else {
            break;
        };
        errors.push(line.to_owned());
        rest = before;
    }
    errors.reverse();
    match rest.split_last() {
        Some((last, before)) if *last == ERRORS => rest = before,
        _ => return Err(bad("missing its error lines label")),
    }
    let request = match rest.split_last() {
        Some((last, before)) => {
            let request = last
                .strip_prefix(ASKED)
                .or_else(|| last.strip_prefix(ASKED_BEFORE_23))
                .and_then(|r| r.strip_suffix('"'))
                .ok_or_else(|| bad("missing the user's request line"))?;
            rest = before;
            request.to_owned()
        }
        None => return Err(bad("missing the user's request line")),
    };
    let summary = rest.join("\n");
    if summary.trim().is_empty() || request.trim().is_empty() {
        return Err(bad("an empty summary or request"));
    }
    let own = format!("session:{}", o.session);
    let mut items = links.split(", ");
    if items.next() != Some(own.as_str()) {
        return Err(bad("its links must start with its session"));
    }
    let mut parsed = Vec::new();
    for item in items {
        parsed.push(
            item.parse::<Related>()
                .map_err(|e| shape(path, format!("occurrence {}: {e}", o.n)))?,
        );
    }
    o.summary = summary;
    o.request = request;
    o.error_lines = errors;
    o.cause = cause;
    o.fix = fix;
    o.links = parsed;
    Ok(())
}
