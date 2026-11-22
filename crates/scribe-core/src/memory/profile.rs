//! `memory/profile.md`: one document, who the user is now
//! (docs/design/scribe/2_design.md §15.5, §15.6). Its seven `## ` headings
//! are fixed here, in order; the weekly job rebuilds it and keeps Pinned
//! byte for byte. No I/O.

use super::{finance_word, sensitive_word};
use std::collections::BTreeSet;

/// The profile's section headings, in order.
pub const HEADINGS: [&str; 7] = [
    "Role and work",
    "Current focus",
    "Projects",
    "Goals",
    "Environment",
    "How they work",
    "Pinned",
];

/// The longest profile the job writes, in characters.
pub const PROFILE_CHARS: usize = 6000;

/// The longest hand-edit diff the job reads, in characters.
pub const DIFF_CHARS: usize = 4000;

/// The heading whose body the job copies unchanged.
pub const PINNED: &str = "Pinned";

/// The profile of a store with none yet: the seven headings, empty.
#[must_use]
pub fn template() -> String {
    let mut out = String::new();
    for heading in HEADINGS {
        out.push_str("## ");
        out.push_str(heading);
        out.push_str("\n\n");
    }
    out
}

/// The profile's sections as (heading, body), when it holds exactly the
/// seven headings in order and nothing before the first; the body is the
/// text after its heading line up to the next heading.
#[must_use]
pub fn sections(text: &str) -> Option<Vec<(&str, &str)>> {
    let mut out: Vec<(&str, &str)> = Vec::new();
    let mut head: Option<&str> = None;
    let mut body_start = 0;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let bare = line.trim_end_matches(['\n', '\r']);
        if let Some(heading) = bare.strip_prefix("## ") {
            match head {
                Some(h) => out.push((h, &text[body_start..offset])),
                None => {
                    if !text[..offset].trim().is_empty() {
                        return None;
                    }
                }
            }
            head = Some(heading.trim_end());
            body_start = offset + line.len();
        }
        offset += line.len();
    }
    out.push((head?, &text[body_start..]));
    let names: Vec<&str> = out.iter().map(|(h, _)| *h).collect();
    (names == HEADINGS).then_some(out)
}

/// The Pinned section's body with its trailing blank lines removed, when the
/// profile has the fixed shape.
#[must_use]
pub fn pinned(text: &str) -> Option<&str> {
    sections(text)?
        .into_iter()
        .find(|(h, _)| *h == PINNED)
        .map(|(_, body)| body.trim_end_matches(['\n', '\r', ' ', '\t']))
}

/// Why the job's profile is refused (§15.6); nothing is written and the
/// cursor stays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileRefusal {
    /// `profile_shape`: not the seven headings, in order.
    Shape,
    /// `profile_pinned`: Pinned changed.
    Pinned,
    /// `profile_too_long`: over [`PROFILE_CHARS`].
    TooLong,
    /// `profile_unknown_link`: a linked entry id that does not exist.
    UnknownLink,
    /// `profile_sensitive`: a credential (the redactor changes it), a
    /// finance word or a sensitive event outside Pinned.
    Sensitive,
}

impl ProfileRefusal {
    /// The stable code.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Shape => "profile_shape",
            Self::Pinned => "profile_pinned",
            Self::TooLong => "profile_too_long",
            Self::UnknownLink => "profile_unknown_link",
            Self::Sensitive => "profile_sensitive",
        }
    }
}

/// Check a rebuilt profile (§15.6): the seven headings in order; Pinned
/// equal to `pinned_before` (trailing blank lines aside); at most
/// [`PROFILE_CHARS`]; every `<tag>-<n>` it names, for a tag in `tags`, in
/// `known`; and outside Pinned, text the redactor leaves alone with no
/// finance or sensitive-event word. Pinned is the user's own text and is
/// not gated.
///
/// # Errors
///
/// The first check that fails.
pub fn check(
    text: &str,
    pinned_before: &str,
    tags: &[String],
    known: &BTreeSet<String>,
    masks: &dyn Fn(&str) -> bool,
) -> Result<(), ProfileRefusal> {
    let parts = sections(text).ok_or(ProfileRefusal::Shape)?;
    let pinned_now = pinned(text).unwrap_or_default();
    if pinned_now != pinned_before.trim_end_matches(['\n', '\r', ' ', '\t']) {
        return Err(ProfileRefusal::Pinned);
    }
    if text.chars().count() > PROFILE_CHARS {
        return Err(ProfileRefusal::TooLong);
    }
    let generated: String = parts
        .iter()
        .filter(|(h, _)| *h != PINNED)
        .map(|(h, body)| format!("## {h}\n{body}"))
        .collect();
    if links(&generated, tags).iter().any(|id| !known.contains(id)) {
        return Err(ProfileRefusal::UnknownLink);
    }
    if masks(&generated) || finance_word(&generated) || sensitive_word(&generated).is_some() {
        return Err(ProfileRefusal::Sensitive);
    }
    Ok(())
}

/// The entry ids `text` names: a word `<tag>-<n>` for a tag in `tags`.
#[must_use]
pub fn links(text: &str, tags: &[String]) -> Vec<String> {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
        .filter(|word| {
            word.rsplit_once('-').is_some_and(|(tag, n)| {
                !n.is_empty()
                    && n.bytes().all(|b| b.is_ascii_digit())
                    && tags.iter().any(|t| t == tag)
            })
        })
        .map(str::to_owned)
        .collect()
}

/// A line diff from `old` to `new`: each removed line as `- `, each added as
/// `+ `, unchanged lines left out; cut at [`DIFF_CHARS`] on a line with
/// `…`. Empty when they are equal.
#[must_use]
pub fn diff(old: &str, new: &str) -> String {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    // Longest common subsequence table, from the end.
    let mut table = vec![vec![0_u32; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1].saturating_add(1)
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut lines: Vec<String> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || table[i][j + 1] >= table[i + 1][j]) {
            lines.push(format!("+ {}", b[j]));
            j += 1;
        } else {
            lines.push(format!("- {}", a[i]));
            i += 1;
        }
    }
    let mut out = String::new();
    for line in lines {
        if out.chars().count() + line.chars().count() + 1 > DIFF_CHARS {
            out.push('…');
            break;
        }
        out.push_str(&line);
        out.push('\n');
    }
    out
}
