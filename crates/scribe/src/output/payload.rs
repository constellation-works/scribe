//! The machine-output contract (STD-01 R6, R10, R11).
//!
//! Every rendering — table, tab-separated, JSON — is derived from these
//! structs, so the human and machine views cannot disagree about what exists.
//! The JSON field names and types are a public contract: renaming, removing
//! or retyping one is a breaking change, pinned by the output goldens. These
//! shapes are deliberately separate from the store's persisted documents.
//! An absent value is `null`, never an omitted key (R11).

use scribe_core::activity::{ActiveChange, CatalogedBy};
use scribe_core::ask::{Answer, Found, Task};
use scribe_core::{
    Author, Child, Cite, ContentFrom, ControlState, Entry, Instruction, Proposal, Quote, Segment,
    SessionControl, SessionId, SessionRecord, Skipped, TagSummary, Term,
};
use serde::Serialize;
use std::collections::BTreeMap;
use time::OffsetDateTime;

/// RFC 3339 with an explicit offset (R11).
type Timestamp = OffsetDateTime;

/// Tag text as output shows it: `{user}` reads "the user", never the
/// configured name, which reaches prompts only (2_design.md §15.8).
fn shown(text: &str) -> String {
    scribe_core::voice::UserName::default().fill(text)
}

/// Where an entry's content came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ContentFromPayload {
    /// `instruction`, `message` or `agent`.
    pub(crate) kind: &'static str,
    /// `user` or `assistant` for a message; `null` for an instruction.
    pub(crate) role: Option<&'static str>,
    /// The message's uuid; `null` for an instruction.
    pub(crate) message: Option<String>,
    /// When the message was sent; `null` for an instruction.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) at: Option<Timestamp>,
}

impl From<&ContentFrom> for ContentFromPayload {
    fn from(from: &ContentFrom) -> Self {
        match from {
            ContentFrom::Agent => Self {
                kind: "agent",
                role: None,
                message: None,
                at: None,
            },
            ContentFrom::Instruction => Self {
                kind: "instruction",
                role: None,
                message: None,
                at: None,
            },
            ContentFrom::Message { role, message, at } => Self {
                kind: "message",
                role: Some(role.as_str()),
                message: Some(message.clone()),
                at: Some(*at),
            },
        }
    }
}

/// Who wrote a record or a change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AuthorPayload {
    /// `daniel`, `agent` or `scribe`.
    pub(crate) kind: &'static str,
    pub(crate) name: String,
}

impl From<&Author> for AuthorPayload {
    fn from(author: &Author) -> Self {
        Self {
            kind: author.kind.as_str(),
            name: author.name.to_string(),
        }
    }
}

/// The last change of a session's `active` or `keep_active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ActiveChangePayload {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) by: AuthorPayload,
    /// `age`, `removed` or `manual`.
    pub(crate) route: &'static str,
    pub(crate) reason: String,
    pub(crate) cite: Option<String>,
}

impl From<&ActiveChange> for ActiveChangePayload {
    fn from(change: &ActiveChange) -> Self {
        Self {
            at: change.at,
            by: AuthorPayload::from(&change.by),
            route: change.route.as_str(),
            reason: change.reason.clone(),
            cite: change.cite.clone(),
        }
    }
}

/// Who cataloged a session by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CatalogedByPayload {
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) by: AuthorPayload,
}

impl From<&CatalogedBy> for CatalogedByPayload {
    fn from(c: &CatalogedBy) -> Self {
        Self {
            at: c.at,
            by: AuthorPayload::from(&c.by),
        }
    }
}

/// One segment of a session's log, with its text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SegmentPayload {
    pub(crate) n: u32,
    pub(crate) heading: String,
    pub(crate) first_message: Option<String>,
    pub(crate) last_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) from: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) to: Timestamp,
    /// `open` or `sealed`.
    pub(crate) state: &'static str,
    pub(crate) windows: u32,
    /// `pass`, `rebuild`, `catalog` or `upgrade`.
    pub(crate) origin: &'static str,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    /// Model-written, never the user's words.
    pub(crate) text: String,
}

impl From<&Segment> for SegmentPayload {
    fn from(s: &Segment) -> Self {
        Self {
            n: s.n.get(),
            heading: s.heading.clone(),
            first_message: s.first_message.clone(),
            last_message: s.last_message.clone(),
            from: s.from,
            to: s.to,
            state: s.state.as_str(),
            windows: s.windows.get(),
            origin: s.origin.as_str(),
            topics: strings(s.topics.as_slice()),
            related: strings(&s.related),
            text: s.text.clone(),
        }
    }
}

/// The turn a record was typed in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CitePayload {
    pub(crate) session: String,
    pub(crate) message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) prompt_sha256: String,
    pub(crate) line: u32,
    pub(crate) cwd: String,
}

impl From<&Cite> for CitePayload {
    fn from(cite: &Cite) -> Self {
        Self {
            session: cite.session.to_string(),
            message: cite.message.clone(),
            at: cite.at,
            prompt_sha256: cite.prompt_sha256.clone(),
            line: cite.line,
            cwd: cite.cwd.clone(),
        }
    }
}

/// The user's checked instruction that authorized an agent write (format 13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct InstructionPayload {
    pub(crate) session: String,
    pub(crate) message: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) quote: Option<String>,
}

impl From<&Instruction> for InstructionPayload {
    fn from(inst: &Instruction) -> Self {
        Self {
            session: inst.session.to_string(),
            message: inst.message.clone(),
            at: inst.at,
            quote: inst.quote.clone(),
        }
    }
}

/// One entry as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct EntryPayload {
    pub(crate) id: String,
    pub(crate) tag: String,
    /// `active`, `superseded`, `retired` or `forgotten`.
    pub(crate) status: &'static str,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    /// The `#scribe` line as typed.
    pub(crate) said: String,
    /// What the entry records, verbatim.
    pub(crate) content: String,
    pub(crate) content_from: ContentFromPayload,
    pub(crate) cite: Option<CitePayload>,
    pub(crate) superseded_by: Option<String>,
    /// `YYYY-MM-DD`.
    pub(crate) review_after: Option<String>,
    /// Who wrote it; `daniel` for a typed `#scribe` line.
    pub(crate) author: AuthorPayload,
    /// Who last updated or retired it through a CLI verb.
    pub(crate) updated_by: Option<AuthorPayload>,
    pub(crate) retirement_reason: Option<String>,
    /// An agent entry's quote of the user's correction, or the user's verbatim
    /// words beside scribe's statement of them (`stated_by` set); `null`
    /// otherwise.
    pub(crate) quote: Option<String>,
    /// A finding's evidence pointer; `null` for other entries.
    pub(crate) evidence: Option<String>,
    /// The date a finding's result was true, `YYYY-MM-DD`; `null` otherwise.
    pub(crate) as_of: Option<String>,
    /// How many turns the entry's transcript window holds; `null` when it
    /// has none (only typed lines under an action tag do).
    pub(crate) window_turns: Option<usize>,
    /// The window itself, only with `--window`; otherwise `null`.
    pub(crate) window: Option<WindowPayload>,
    /// For an entry under an action tag, where it stands for an intake
    /// (`entry list`, `entry show`, `entry add`, `entry pickup`); `null`
    /// for any other entry and in other commands' output.
    pub(crate) action: Option<ActionPayload>,
    /// For an entry approved from a pass claim, what admitted the claim
    /// (`entry show` only); `null` for any other entry and in other
    /// commands' output.
    pub(crate) admitted: Option<scribe_core::Admitted>,
    /// The agent that approved this candidate on the user's instruction; `null` otherwise.
    pub(crate) approved_by: Option<AuthorPayload>,
    /// The user's checked instruction `approved_by` approved this candidate
    /// on; `null` otherwise. A later retirement or forgetting never
    /// replaces it.
    pub(crate) instruction: Option<InstructionPayload>,
    /// Whose words `content` is when it is a statement of a quoted turn:
    /// `{kind: scribe, name: session-pass}` or `{kind: scribe, name: annotation}`
    /// when scribe wrote it, or `{kind: agent, name: <author>}` when an agent
    /// wrote it on the user's spoken instruction (their words are then in
    /// `quote`); `null` otherwise.
    pub(crate) stated_by: Option<AuthorPayload>,
    /// Where an idea the session pass captured itself stands: `{state:
    /// open|parked|task, reason, task}`, `reason` the user's own words
    /// parking it; `null` for every other entry.
    pub(crate) outcome: Option<scribe_core::Outcome>,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
    pub(crate) decision: Option<scribe_core::Decision>,
    pub(crate) item: Option<scribe_core::OpenItem>,
    /// For an entry under `open-item`, its derived state: `open`,
    /// `answered`, `closed` or `withdrawn` (2_design.md §18.4); `null`
    /// for every other entry.
    pub(crate) state: Option<&'static str>,
    /// The user's checked instruction that retired or forgot this entry;
    /// `null` otherwise.
    pub(crate) removal_instruction: Option<InstructionPayload>,
}

/// An action entry's transcript window (docs/design/scribe/2_design.md §4.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WindowPayload {
    pub(crate) turns: Vec<WindowTurnPayload>,
    /// Whether older turns were dropped to fit the cap.
    pub(crate) truncated: bool,
    /// How many older turns were dropped.
    pub(crate) omitted: u32,
}

/// One turn of a window.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WindowTurnPayload {
    /// `user` or `assistant`.
    pub(crate) role: &'static str,
    /// The transcript message's uuid; `null` for the prompt the line was
    /// typed in.
    pub(crate) message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    pub(crate) text: String,
    /// Whether `text` was cut.
    pub(crate) cut: bool,
}

impl From<&scribe_core::Window> for WindowPayload {
    fn from(window: &scribe_core::Window) -> Self {
        Self {
            turns: window
                .turns
                .iter()
                .map(|t| WindowTurnPayload {
                    role: t.role.as_str(),
                    message: t.message.clone(),
                    at: t.at,
                    text: t.text.clone(),
                    cut: t.cut,
                })
                .collect(),
            truncated: window.truncated,
            omitted: window.omitted,
        }
    }
}

/// An action entry's state for an intake, at the time of the command.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ActionPayload {
    /// Seconds since it was created.
    pub(crate) age_seconds: i64,
    /// When an agent-written entry's cancel window ends; `null` for
    /// the user's, which have none.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) cancel_window_ends: Option<Timestamp>,
    /// Whether an intake may take it now.
    pub(crate) past_cancel_window: bool,
    /// Whether a consumer recorded a pickup.
    pub(crate) picked_up: bool,
    /// Whether a consumer recorded that the user rejected its result.
    pub(crate) rejected: bool,
    pub(crate) pickups: Vec<PickupPayload>,
}

/// A consumer's note on an action entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PickupPayload {
    pub(crate) entry: String,
    pub(crate) author: AuthorPayload,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    /// What the consumer made of it, such as `task:<id>`; `null` when it
    /// named nothing.
    pub(crate) result: Option<String>,
    pub(crate) rejected: bool,
}

impl From<&scribe_core::action::Pickup> for PickupPayload {
    fn from(p: &scribe_core::action::Pickup) -> Self {
        Self {
            entry: p.entry.to_string(),
            author: AuthorPayload::from(&p.author),
            at: p.at,
            result: p.result.as_ref().map(ToString::to_string),
            rejected: p.rejected,
        }
    }
}

impl From<&scribe_core::action::ActionState> for ActionPayload {
    fn from(state: &scribe_core::action::ActionState) -> Self {
        Self {
            age_seconds: state.age.whole_seconds(),
            cancel_window_ends: state.cancel_window_ends,
            past_cancel_window: state.past_cancel_window,
            picked_up: state.picked_up,
            rejected: state.rejected,
            pickups: state.pickups.iter().map(PickupPayload::from).collect(),
        }
    }
}

impl EntryPayload {
    /// With the action state an `entry` command computed.
    pub(crate) fn with_action(mut self, state: Option<&scribe_core::action::ActionState>) -> Self {
        self.action = state.map(ActionPayload::from);
        self
    }

    /// With the admission of the claim it was approved from, for `entry show`.
    pub(crate) fn with_admitted(mut self, admitted: Option<scribe_core::Admitted>) -> Self {
        self.admitted = admitted;
        self
    }

    /// With the window included, for `--window`.
    pub(crate) fn with_window(mut self, entry: &Entry) -> Self {
        self.window = entry.window.as_ref().map(WindowPayload::from);
        self
    }
}

fn strings<T: ToString>(items: &[T]) -> Vec<String> {
    items.iter().map(ToString::to_string).collect()
}

/// `YYYY-MM-DD`.
fn date(date: time::Date) -> String {
    format!(
        "{:04}-{:02}-{:02}",
        date.year(),
        u8::from(date.month()),
        date.day()
    )
}

impl From<&Entry> for EntryPayload {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.to_string(),
            tag: entry.tag.to_string(),
            status: entry.status.as_str(),
            project: entry.project.to_string(),
            topics: strings(entry.topics.as_slice()),
            related: strings(&entry.related),
            provider: entry.provider.to_string(),
            host: entry.host.to_string(),
            created: entry.created,
            updated: entry.updated,
            said: entry.said.clone(),
            content: entry.content.clone(),
            content_from: ContentFromPayload::from(&entry.content_from),
            cite: entry.cite.as_ref().map(CitePayload::from),
            superseded_by: entry.superseded_by.as_ref().map(ToString::to_string),
            review_after: entry.review_after.map(date),
            author: AuthorPayload::from(&entry.author),
            updated_by: entry.updated_by.as_ref().map(AuthorPayload::from),
            retirement_reason: entry.retirement_reason.clone(),
            quote: entry.quote.clone(),
            evidence: entry.evidence.clone(),
            as_of: entry.as_of.map(date),
            window_turns: entry.window.as_ref().map(|w| w.turns.len()),
            window: None,
            action: None,
            admitted: None,
            approved_by: entry.approved_by.as_ref().map(AuthorPayload::from),
            instruction: entry.instruction.as_ref().map(InstructionPayload::from),
            stated_by: entry.stated_by.as_ref().map(AuthorPayload::from),
            outcome: entry.outcome.clone(),
            handoff_mode: entry.handoff_mode,
            decision: entry.decision.clone(),
            item: entry.item.clone(),
            state: scribe_core::ItemState::of(entry).map(scribe_core::ItemState::as_str),
            removal_instruction: entry
                .removal_instruction
                .as_ref()
                .map(InstructionPayload::from),
        }
    }
}

/// A phrase the user coined or defined, with their defining words.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TermPayload {
    pub(crate) term: String,
    /// Verbatim from the user's turn.
    pub(crate) said: String,
    /// The uuid of that turn's message.
    pub(crate) message: String,
}

impl From<&Term> for TermPayload {
    fn from(t: &Term) -> Self {
        Self {
            term: t.term.clone(),
            said: t.said.clone(),
            message: t.message.clone(),
        }
    }
}

/// A link a session record stores: how it relates to another session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct LinkPayload {
    /// `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The session linked to.
    pub(crate) session: String,
    /// That session's record title; `null` when it has no record.
    pub(crate) title: Option<String>,
}

/// A session that links to this one, derived at read time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ChildPayload {
    /// How the child relates to this session: `spawned_from` or `continues`.
    #[serde(rename = "type")]
    pub(crate) kind: &'static str,
    /// The child session.
    pub(crate) session: String,
    /// Its record title.
    pub(crate) title: String,
}

impl From<&Child> for ChildPayload {
    fn from(child: &Child) -> Self {
        Self {
            kind: child.kind.as_str(),
            session: child.session.to_string(),
            title: child.title.clone(),
        }
    }
}

/// What a session payload shows besides its own record: the sessions that
/// link to it and the titles of the records it links to.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Kin<'a> {
    pub(crate) children: &'a [Child],
    pub(crate) titles: &'a BTreeMap<SessionId, String>,
}

impl<'a> Kin<'a> {
    /// `id`'s kin from a listing's derived children and titles.
    pub(crate) fn of(
        id: &SessionId,
        children: &'a BTreeMap<SessionId, Vec<Child>>,
        titles: &'a BTreeMap<SessionId, String>,
    ) -> Self {
        Self {
            children: children.get(id).map_or(&[][..], Vec::as_slice),
            titles,
        }
    }
}

/// One agent-written handoff entry shown beside a session's header
/// (docs/design/scribe/2_design.md §16).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct HandoffPayload {
    pub(crate) id: String,
    pub(crate) author: AuthorPayload,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    pub(crate) project: String,
    pub(crate) quote: Option<String>,
    pub(crate) content: String,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
}

impl From<&Entry> for HandoffPayload {
    fn from(entry: &Entry) -> Self {
        Self {
            id: entry.id.to_string(),
            author: AuthorPayload::from(&entry.author),
            created: entry.created,
            project: entry.project.to_string(),
            quote: entry.quote.clone(),
            content: entry.content.clone(),
            handoff_mode: entry.handoff_mode,
        }
    }
}

/// `session show <id>` for a session with no session record, only
/// agent-written handoffs (2_design.md §16). Carries its switch state when
/// switched on before its first pass, and never fabricates record timestamps.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionHandoffsPayload {
    pub(crate) id: String,
    pub(crate) has_record: bool,
    pub(crate) project: String,
    pub(crate) on: bool,
    pub(crate) due: bool,
    pub(crate) summary: String,
    pub(crate) goals: String,
    pub(crate) next: String,
    pub(crate) handoffs: Vec<HandoffPayload>,
}

impl SessionHandoffsPayload {
    pub(crate) fn new(
        id: &SessionId,
        control: Option<&SessionControl>,
        handoffs: &[Entry],
    ) -> Self {
        let project = handoffs
            .first()
            .map_or_else(|| String::from("global"), |e| e.project.to_string());
        Self {
            id: id.to_string(),
            has_record: false,
            project,
            on: control.is_some_and(|c| c.on),
            due: control.is_some_and(|c| c.due),
            summary: String::new(),
            goals: String::new(),
            next: String::new(),
            handoffs: handoffs.iter().map(HandoffPayload::from).collect(),
        }
    }
}

/// One session record, with its switch, as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SessionPayload {
    pub(crate) id: String,
    /// The session's `s-<n>` (2_design.md §15.7), accepted wherever an id
    /// is; `id` stays the provider's session id, which cites use.
    pub(crate) short_id: Option<String>,
    /// `open` or `final`.
    pub(crate) status: &'static str,
    /// When `#scribe forget` erased the session's words (2_design.md
    /// §15.7), or `null`.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) forgotten: Option<Timestamp>,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    pub(crate) related: Vec<String>,
    /// The links this session stores (`spawned_from`, `continues`).
    pub(crate) links: Vec<LinkPayload>,
    /// The sessions that link to this one, derived at read time.
    pub(crate) children: Vec<ChildPayload>,
    /// The user's own words, unlike the four model-written sections.
    pub(crate) terms: Vec<TermPayload>,
    pub(crate) provider: String,
    pub(crate) host: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    pub(crate) title: String,
    pub(crate) cwd: String,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) started: Timestamp,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) on_since: Timestamp,
    pub(crate) scribed_through: Option<String>,
    pub(crate) passes: u32,
    pub(crate) next_open: u32,
    /// Model-written, never the user's words.
    pub(crate) summary: String,
    pub(crate) goals: String,
    /// Always `""` from store format 4: what it held is in `log`. Kept so
    /// no reader of the field breaks.
    pub(crate) outcome: String,
    pub(crate) next: String,
    /// Whether cataloging is on now (`control.json`); `false` when the
    /// session has no switch.
    pub(crate) on: bool,
    /// Whether a pass is due and not yet run.
    pub(crate) due: bool,
    /// Whether the record still describes something current.
    pub(crate) active: bool,
    /// Exempt from automatic deactivation.
    pub(crate) keep_active: bool,
    pub(crate) active_change: Option<ActiveChangePayload>,
    pub(crate) cataloged_by: Option<CatalogedByPayload>,
    /// The agent that ran `session rebuild` to write this record, or `null`.
    pub(crate) rebuilt_by: Option<AuthorPayload>,
    /// The log's segments with their text, oldest first (one with
    /// `--segment`); absent from a list, whose rows carry no log.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) log: Option<Vec<SegmentPayload>>,
    /// Active agent-written handoffs for this session, newest first; absent
    /// from a list, whose rows carry no handoffs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) handoffs: Option<Vec<HandoffPayload>>,
    /// Whether the table view prints each segment's text (`--log`,
    /// `--segment`) or only the table of contents.
    #[serde(skip)]
    pub(crate) log_text: bool,
}

impl SessionPayload {
    pub(crate) fn new(
        record: &SessionRecord,
        control: Option<&SessionControl>,
        kin: Kin<'_>,
    ) -> Self {
        Self {
            id: record.id.to_string(),
            short_id: None,
            status: record.status.as_str(),
            forgotten: None,
            project: record.project.to_string(),
            topics: strings(record.topics.as_slice()),
            related: strings(&record.related),
            links: record
                .links
                .iter()
                .map(|l| LinkPayload {
                    kind: l.kind.as_str(),
                    session: l.session.to_string(),
                    title: kin.titles.get(&l.session).cloned(),
                })
                .collect(),
            children: kin.children.iter().map(ChildPayload::from).collect(),
            terms: record.terms.iter().map(TermPayload::from).collect(),
            provider: record.provider.to_string(),
            host: record.host.to_string(),
            created: record.created,
            updated: record.updated,
            title: record.title.clone(),
            cwd: record.cwd.clone(),
            started: record.started,
            on_since: record.on_since,
            scribed_through: record.scribed_through.clone(),
            passes: record.passes,
            next_open: record.next_open,
            summary: record.summary.clone(),
            goals: record.goals.clone(),
            outcome: String::new(),
            next: record.next.clone(),
            on: control.is_some_and(|c| c.on),
            due: control.is_some_and(|c| c.due),
            active: record.active,
            keep_active: record.keep_active,
            active_change: record.active_change.as_ref().map(ActiveChangePayload::from),
            cataloged_by: record.cataloged_by.as_ref().map(CatalogedByPayload::from),
            rebuilt_by: record.rebuilt_by.as_ref().map(AuthorPayload::from),
            log: None,
            handoffs: None,
            log_text: false,
        }
    }

    /// With its active handoff entries, newest first, for `session show`.
    pub(crate) fn with_handoffs<'a>(
        mut self,
        handoffs: impl IntoIterator<Item = &'a Entry>,
    ) -> Self {
        self.handoffs = Some(handoffs.into_iter().map(HandoffPayload::from).collect());
        self
    }

    /// With when it was forgotten.
    pub(crate) fn with_forgotten(mut self, at: Option<Timestamp>) -> Self {
        self.forgotten = at;
        self
    }

    /// With its `s-<n>`.
    pub(crate) fn with_number(mut self, number: Option<std::num::NonZeroU64>) -> Self {
        self.short_id = number.map(|n| format!("s-{n}"));
        self
    }

    /// With `segments` as its `log`: a `show` carries the whole log, or the
    /// one segment asked for.
    pub(crate) fn with_log<'a>(mut self, segments: impl IntoIterator<Item = &'a Segment>) -> Self {
        self.log = Some(segments.into_iter().map(SegmentPayload::from).collect());
        self
    }
}

/// A verbatim span a proposal rests on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct QuotePayload {
    pub(crate) entry: String,
    pub(crate) text: String,
}

impl From<&Quote> for QuotePayload {
    fn from(quote: &Quote) -> Self {
        Self {
            entry: quote.entry.to_string(),
            text: quote.text.clone(),
        }
    }
}

/// One proposal as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProposalPayload {
    pub(crate) candidate: Option<scribe_core::EntryCandidate>,
    pub(crate) id: String,
    /// `supersedes`, `stale` or `candidate`.
    pub(crate) kind: &'static str,
    pub(crate) older: String,
    pub(crate) newer: Option<String>,
    /// Model text.
    pub(crate) reason: String,
    pub(crate) quotes: Vec<QuotePayload>,
    /// `open`, `approved` or `rejected`.
    pub(crate) state: &'static str,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) opened_at: Timestamp,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) settled_at: Option<Timestamp>,
    pub(crate) settled_by: Option<CitePayload>,
    /// Who opened it.
    pub(crate) author: AuthorPayload,
    /// Who settled it; `null` while open.
    pub(crate) settled_author: Option<AuthorPayload>,
    pub(crate) updated_by: Option<AuthorPayload>,
    /// Why scribe closed it without the user (`tag_retired`, `instructed`); `null` otherwise.
    pub(crate) closed_reason: Option<&'static str>,
    /// The user's checked instruction that settled this candidate; `null` otherwise.
    pub(crate) instruction: Option<InstructionPayload>,
}

impl From<&Proposal> for ProposalPayload {
    fn from(p: &Proposal) -> Self {
        Self {
            id: p.id.to_string(),
            kind: p.kind.as_str(),
            candidate: p.candidate.clone(),
            older: p.older.to_string(),
            newer: p.newer.as_ref().map(ToString::to_string),
            reason: p.reason.clone(),
            quotes: p.quotes.iter().map(QuotePayload::from).collect(),
            state: p.state.as_str(),
            opened_at: p.opened_at,
            settled_at: p.settled_at,
            settled_by: p.settled_by.as_ref().map(CitePayload::from),
            author: AuthorPayload::from(&p.author),
            settled_author: p.settled_author.as_ref().map(AuthorPayload::from),
            updated_by: p.updated_by.as_ref().map(AuthorPayload::from),
            closed_reason: p.closed_reason.map(scribe_core::ClosedReason::as_str),
            instruction: p.instruction.as_ref().map(InstructionPayload::from),
        }
    }
}

/// The label an older claim's statement carries where a context would be
/// (2_design.md §15.3).
pub(crate) const OLDER_CLAIM: &str = "older claim";

/// What a claim says, for the outputs and the popup (2_design.md §15.3):
/// its context paragraph, or an older claim's one-line statement labelled
/// [`OLDER_CLAIM`]; `None` for a claim opened before statements.
pub(crate) fn claim_context(candidate: &scribe_core::EntryCandidate) -> Option<String> {
    match (&candidate.context, &candidate.statement) {
        (Some(context), _) => Some(context.clone()),
        (None, Some(statement)) => Some(format!("{OLDER_CLAIM}: {statement}")),
        (None, None) => None,
    }
}

/// One tag as every output mode sees it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagPayload {
    pub(crate) author: Option<AuthorPayload>,
    pub(crate) source: Option<&'static str>,
    pub(crate) name: String,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: Option<String>,
    /// `null` when entries never go stale.
    pub(crate) stale_after_days: Option<u32>,
    /// Whether `config.toml` (or the defaults) declares it; `false` for a
    /// directory of entries no configured tag names.
    pub(crate) configured: bool,
    /// Entries in it, any status.
    pub(crate) entries: usize,
    /// For an action tag, how agents use it and how often the user undid
    /// their entries; `null` for a memory tag.
    pub(crate) action: Option<TagActionPayload>,
    /// The capture route (`propose` or `write`); `null` for a tag
    /// with no capture rules, which the pass never captures.
    pub(crate) route: Option<&'static str>,
}

/// An action tag's switch and counts, where the user decides to flip it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagActionPayload {
    /// `free` (agents write unasked) or `ask-first`.
    pub(crate) mode: &'static str,
    /// Whether an agent's entry must quote the user's correction.
    pub(crate) require_quote: bool,
    /// Agent-written entries under it that were retired or forgotten.
    pub(crate) retired: usize,
    /// Agent-written entries under it whose result was rejected.
    pub(crate) rejected: usize,
}

impl From<&TagSummary> for TagPayload {
    fn from(tag: &TagSummary) -> Self {
        Self {
            source: tag.source,
            author: tag.author.as_ref().map(AuthorPayload::from),
            name: tag.name.to_string(),
            aliases: tag
                .def
                .as_ref()
                .map(|d| strings(&d.aliases))
                .unwrap_or_default(),
            description: tag
                .def
                .as_ref()
                .and_then(|d| d.description.as_deref().map(shown)),
            stale_after_days: tag
                .def
                .as_ref()
                .and_then(|d| d.stale_after_days)
                .map(|d| d.get()),
            configured: tag.source == Some("config"),
            entries: tag.entries,
            action: tag
                .def
                .as_ref()
                .and_then(|d| d.action)
                .map(|a| TagActionPayload {
                    mode: a.mode(),
                    require_quote: a.require_quote,
                    retired: tag.retired,
                    rejected: tag.rejected,
                }),
            route: tag
                .def
                .as_ref()
                .and_then(|d| d.capture.route())
                .map(scribe_core::CaptureRoute::as_str),
        }
    }
}

/// `tag show`: a tag's settings and its effective capture rules, each
/// field with its origin (docs/design/scribe/2_design.md §11.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagShowPayload {
    pub(crate) tag: String,
    /// `config` (config.toml or the shipped defaults) or `store`.
    pub(crate) source: &'static str,
    pub(crate) aliases: Vec<String>,
    pub(crate) description: Option<String>,
    pub(crate) stale_after_days: Option<u32>,
    /// An action tag's settings; `null` for a memory tag.
    pub(crate) action: Option<TagShowActionPayload>,
    pub(crate) capture: CapturePayload,
}

/// An action tag's settings, as `tag show` prints them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TagShowActionPayload {
    /// `free` or `ask-first`.
    pub(crate) mode: &'static str,
    pub(crate) require_quote: bool,
}

/// A tag's effective capture rules; a field with origin `none` is `null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CapturePayload {
    pub(crate) from: Option<&'static str>,
    pub(crate) route: Option<&'static str>,
    pub(crate) triggers: Option<Vec<String>>,
    pub(crate) criteria: Option<String>,
    pub(crate) not: Option<String>,
    /// The most open claims the tag may hold: the config's value, else
    /// the default 10 for a tag with a route; `null` for a tag that
    /// catches nothing.
    pub(crate) max_open: Option<u32>,
    pub(crate) origin: CaptureOriginPayload,
}

/// Where each capture field came from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CaptureOriginPayload {
    pub(crate) from: OriginPayload,
    pub(crate) route: OriginPayload,
    pub(crate) triggers: OriginPayload,
    pub(crate) criteria: OriginPayload,
    pub(crate) not: OriginPayload,
    pub(crate) max_open: OriginPayload,
}

/// One field's origin: `shipped`, `config` (with the file and line) or
/// `none`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OriginPayload {
    pub(crate) source: &'static str,
    /// The config file, for `config`; `null` otherwise.
    pub(crate) file: Option<String>,
    /// The 1-based line of the key, for `config`; `null` otherwise.
    pub(crate) line: Option<usize>,
}

impl OriginPayload {
    /// A default scribe ships.
    const SHIPPED: Self = Self {
        source: "shipped",
        file: None,
        line: None,
    };

    fn of<T>(rule: Option<&scribe_core::Rule<T>>, file: &str) -> Self {
        match rule.map(|r| r.origin) {
            None => Self {
                source: "none",
                file: None,
                line: None,
            },
            Some(scribe_core::RuleOrigin::Shipped) => Self {
                source: "shipped",
                file: None,
                line: None,
            },
            Some(scribe_core::RuleOrigin::Config { line }) => Self {
                source: "config",
                file: Some(file.to_owned()),
                line,
            },
        }
    }
}

impl TagShowPayload {
    /// The payload for `def`, read from `source`, whose config file is
    /// `config_file`.
    pub(crate) fn new(def: &scribe_core::TagDef, source: &'static str, config_file: &str) -> Self {
        let c = &def.capture;
        Self {
            tag: def.name.to_string(),
            source,
            aliases: strings(&def.aliases),
            description: def.description.as_deref().map(shown),
            stale_after_days: def.stale_after_days.map(std::num::NonZeroU32::get),
            action: def.action.map(|a| TagShowActionPayload {
                mode: a.mode(),
                require_quote: a.require_quote,
            }),
            capture: CapturePayload {
                from: c.from.as_ref().map(|r| r.value.as_str()),
                route: c.route.as_ref().map(|r| r.value.as_str()),
                triggers: c.triggers.as_ref().map(|r| r.value.clone()),
                criteria: c.criteria.as_ref().map(|r| shown(&r.value)),
                not: c.not.as_ref().map(|r| shown(&r.value)),
                max_open: (c.max_open.is_some() || c.route.is_some()).then(|| c.max_open()),
                origin: CaptureOriginPayload {
                    from: OriginPayload::of(c.from.as_ref(), config_file),
                    route: OriginPayload::of(c.route.as_ref(), config_file),
                    triggers: OriginPayload::of(c.triggers.as_ref(), config_file),
                    criteria: OriginPayload::of(c.criteria.as_ref(), config_file),
                    not: OriginPayload::of(c.not.as_ref(), config_file),
                    max_open: match (&c.max_open, &c.route) {
                        (Some(rule), _) => OriginPayload::of(Some(rule), config_file),
                        (None, Some(_)) => OriginPayload::SHIPPED,
                        (None, None) => OriginPayload::of::<()>(None, config_file),
                    },
                },
            },
        }
    }
}

/// A record file a list left out because it did not parse.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct WarningPayload {
    /// The file.
    pub(crate) path: String,
    /// The stable error code, `store_corrupt`.
    pub(crate) code: &'static str,
    /// What was wrong, naming the file.
    pub(crate) error: String,
}

impl From<&Skipped> for WarningPayload {
    fn from(skipped: &Skipped) -> Self {
        let mut error = skipped.error.to_string();
        let mut source = std::error::Error::source(&skipped.error);
        while let Some(cause) = source {
            error.push_str(": ");
            error.push_str(&cause.to_string());
            source = cause.source();
        }
        Self {
            path: skipped.path.display().to_string(),
            code: skipped.error.code(),
            error,
        }
    }
}

/// The records a list returned, by kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Records {
    Groups(Vec<scribe_core::group::Group>),
    Entries(Vec<EntryPayload>),
    /// `session list`; `active_column` with `--include-inactive`, which adds
    /// an `active` column to the human forms.
    Sessions {
        items: Vec<SessionPayload>,
        active_column: bool,
    },
    /// `session list --tree`: sessions in tree order, each with its depth.
    SessionTree {
        rows: Vec<TreeRow>,
        active_column: bool,
    },
    Proposals(Vec<ProposalPayload>),
    Tags(Vec<TagPayload>),
    /// `scribe search`, best first.
    Hits(Vec<HitPayload>),
    /// `transcript list`.
    Transcripts(Vec<TranscriptPayload>),
    /// `mark list` and `mark retry`: `#scribe` markers as `marks.json`
    /// stores them.
    Marks(Vec<scribe_core::annotation::MarkRecord>),
}

impl Records {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Groups(v) => v.len(),
            Self::Entries(v) => v.len(),
            Self::Sessions { items, .. } => items.len(),
            Self::SessionTree { rows, .. } => rows.len(),
            Self::Proposals(v) => v.len(),
            Self::Tags(v) => v.len(),
            Self::Hits(v) => v.len(),
            Self::Transcripts(v) => v.len(),
            Self::Marks(v) => v.len(),
        }
    }
}

/// One `scribe search` hit (docs/design/scribe/specs/output-contract.md).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct HitPayload {
    /// `entry`, `session` or `message`.
    pub(crate) kind: &'static str,
    /// What `ask` would cite: an entry id, `session:<id>` or `session:<id>#<n>`
    /// (a message cites its session).
    pub(crate) cite: String,
    /// The entry's or the session's id.
    pub(crate) id: String,
    /// The segment's number, or `null` for an entry or a session header.
    pub(crate) segment: Option<u32>,
    /// A stored message's provider id; `null` for any other kind.
    pub(crate) message: Option<String>,
    /// The BM25 score times the unit's weight, to three decimals.
    pub(crate) score: serde_json::Number,
    pub(crate) title: String,
    /// Up to 240 characters around the first matching word, on one line.
    pub(crate) snippet: String,
    pub(crate) project: String,
    pub(crate) topics: Vec<String>,
    /// An entry's author; `null` for a session.
    pub(crate) author: Option<AuthorPayload>,
    /// Whether the session is active; `null` for an entry.
    pub(crate) active: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) updated: Timestamp,
    /// The entry's status (`active`, `superseded`, `retired`) or the
    /// session's (`open`, `final`); added beside the designed fields.
    pub(crate) status: &'static str,
    /// A finding entry's evidence pointer; `null` otherwise.
    pub(crate) evidence: Option<String>,
    /// The date a finding's result was true, `YYYY-MM-DD`; `null` otherwise.
    pub(crate) as_of: Option<String>,
}

impl From<&scribe_core::search::Hit> for HitPayload {
    fn from(hit: &scribe_core::search::Hit) -> Self {
        Self {
            kind: hit.kind.as_str(),
            cite: hit.cite.clone(),
            id: hit.id.clone(),
            segment: hit.segment,
            message: hit.message.clone(),
            score: serde_json::Number::from_f64(hit.score)
                .unwrap_or_else(|| serde_json::Number::from(0)),
            title: hit.title.clone(),
            snippet: hit.snippet.clone(),
            project: hit.project.to_string(),
            topics: strings(&hit.topics),
            author: hit.author.as_ref().map(AuthorPayload::from),
            active: hit.active,
            updated: hit.updated,
            status: hit.status,
            evidence: hit.evidence.clone(),
            as_of: hit.as_of.map(date),
        }
    }
}

/// One row of `session list --tree`. Its JSON is the session's alone: the
/// nesting is in each session's `links` and `children`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct TreeRow {
    /// How deep it nests: 0 for a root.
    #[serde(skip)]
    pub(crate) depth: usize,
    /// The session it nests under, if it has one in the list.
    #[serde(skip)]
    pub(crate) parent: Option<String>,
    pub(crate) session: SessionPayload,
}

/// One record a `show` returned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Record {
    Group(Box<scribe_core::group::Group>),
    Tag(Box<scribe_core::StoreTag>),
    TagRules(Box<TagShowPayload>),
    Proposal(Box<ProposalPayload>),
    Entry(Box<EntryPayload>),
    Session(Box<SessionPayload>),
    /// `session show <id>` for a session with handoff(s) and no session record.
    SessionHandoffs(Box<SessionHandoffsPayload>),
    Transcript(Box<TranscriptPayload>),
    /// One memory file.
    Memory(Box<MemoryPayload>),
    /// `memory show` with no id: the memory block.
    MemoryBlock(Box<MemoryBlockPayload>),
    /// `memory show --revision`.
    MemoryRevision(Box<RevisionPayload>),
    /// `memory refresh`.
    MemoryRefresh(Box<RefreshPayload>),
    /// `store migrate`: the archives imported into `scribe.db`.
    StoreMigrate(Box<MigratePayload>),
    /// `export`: what was written where.
    Export(Box<ExportPayload>),
    /// `memory rules`.
    MemoryRules(Box<MemoryRulesPayload>),
    /// `brief`.
    Brief(Box<BriefPayload>),
}

/// One memory file (`memory show <id>`, and the writes on one;
/// docs/design/scribe/specs/output-contract.md). Field names are a
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryPayload {
    /// `profile`, or the file stem of a `now` item or milestone.
    pub(crate) id: String,
    /// `profile`, `now` or `milestone`.
    pub(crate) kind: &'static str,
    /// The event date or deadline, `YYYY-MM-DD`; `null` for the profile.
    pub(crate) date: Option<String>,
    /// `now` only: `deadline`, `event` or `plan`; `null` otherwise.
    pub(crate) now_kind: Option<&'static str>,
    /// The title; `null` for the profile.
    pub(crate) title: Option<String>,
    /// The prose, or the whole profile.
    pub(crate) text: String,
    /// The sessions it came from; empty for the profile.
    pub(crate) sources: Vec<String>,
    /// When it last changed; `null` for the profile.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) updated: Option<Timestamp>,
    /// `now` only: when an agent last raised it; `null` when never.
    pub(crate) raised: Option<scribe_core::memory::Raised>,
    /// The profile only: the date of its last revision; `null` otherwise.
    pub(crate) as_of: Option<String>,
}

fn day(date: time::Date) -> String {
    date.format(time::macros::format_description!("[year]-[month]-[day]"))
        .unwrap_or_default()
}

impl MemoryPayload {
    /// The payload of a shown file.
    pub(crate) fn from_shown(shown: &scribe_core::MemoryShown) -> Self {
        match shown {
            scribe_core::MemoryShown::Profile(p) => Self::profile(p),
            scribe_core::MemoryShown::Now(n) => Self::now(n),
            scribe_core::MemoryShown::Milestone(m) => Self::milestone(m),
        }
    }

    pub(crate) fn profile(p: &scribe_core::memory::ProfileView) -> Self {
        Self {
            id: String::from("profile"),
            kind: "profile",
            date: None,
            now_kind: None,
            title: None,
            text: p.text.clone(),
            sources: Vec::new(),
            updated: None,
            raised: None,
            as_of: p.as_of.map(day),
        }
    }

    pub(crate) fn now(n: &scribe_core::memory::NowItem) -> Self {
        Self {
            id: n.stem.to_string(),
            kind: "now",
            date: Some(day(n.date)),
            now_kind: Some(n.kind.as_str()),
            title: Some(n.title.clone()),
            text: n.text.clone(),
            sources: n.sources.iter().map(ToString::to_string).collect(),
            updated: Some(n.updated),
            raised: n.raised.clone(),
            as_of: None,
        }
    }

    pub(crate) fn milestone(m: &scribe_core::memory::Milestone) -> Self {
        Self {
            id: m.stem.to_string(),
            kind: "milestone",
            date: Some(day(m.date)),
            now_kind: None,
            title: Some(m.title.clone()),
            text: m.text.clone(),
            sources: m.sources.iter().map(ToString::to_string).collect(),
            updated: Some(m.updated),
            raised: None,
            as_of: None,
        }
    }
}

/// The profile in the memory block.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ProfileBlockPayload {
    /// `profile.md`, or its committed revision while a journal is open.
    pub(crate) text: String,
    /// The date of its last revision; `null` for a file no revision wrote.
    pub(crate) as_of: Option<String>,
}

/// One line of the memory block's Now or Milestones part.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryLinePayload {
    /// The file stem.
    pub(crate) id: String,
    /// `YYYY-MM-DD`.
    pub(crate) date: String,
    /// Its title.
    pub(crate) title: String,
}

/// The memory block (2_design.md §15.4): `memory show` with no id prints
/// it, and `scribe brief` embeds it. `--json` prints `{profile, now,
/// milestones}`. Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryBlockPayload {
    /// The profile; `null` when there is none.
    pub(crate) profile: Option<ProfileBlockPayload>,
    /// Every `now` item, nearest date first.
    pub(crate) now: Vec<MemoryLinePayload>,
    /// The last five milestones, newest first.
    pub(crate) milestones: Vec<MemoryLinePayload>,
    /// The block as text.
    #[serde(skip)]
    pub(crate) text: String,
    /// Warning codes and lines for stderr: `memory_pending`, a read-only
    /// copy, files that did not parse, items not yet migrated.
    #[serde(skip)]
    pub(crate) warnings: Vec<String>,
}

impl MemoryBlockPayload {
    /// The block of `view`, with the root it was read from for notices.
    pub(crate) fn from_view(view: &scribe_core::MemoryView, root: &std::path::Path) -> Self {
        let mut now: Vec<&scribe_core::memory::NowItem> = view.now.iter().collect();
        now.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.stem.cmp(&b.stem)));
        let mut warnings = Vec::new();
        if view.pending {
            warnings.push(String::from(
                "warning: memory_pending: a memory write was interrupted; the profile shown is its committed revision, and the next memory write completes it",
            ));
        }
        if view.read_only_copy {
            warnings.push(format!(
                "read-only copy: {} has no -wal or -shm file and its directory is read-only, so it was opened immutable",
                root.join(scribe_core::DB_FILE).display()
            ));
        }
        for skipped in &view.skipped {
            warnings.push(format!("warning: skipped {}", skipped.error));
        }
        if view.unmigrated > 0 {
            warnings.push(format!(
                "warning: {} format-24 memory item(s) not read; the next store write archives them",
                view.unmigrated
            ));
        }
        Self {
            profile: view.profile.as_ref().map(|p| ProfileBlockPayload {
                text: p.text.clone(),
                as_of: p.as_of.map(day),
            }),
            now: now
                .iter()
                .map(|n| MemoryLinePayload {
                    id: n.stem.to_string(),
                    date: day(n.date),
                    title: n.title.clone(),
                })
                .collect(),
            milestones: scribe_core::memory::newest_milestones(&view.milestones)
                .into_iter()
                .map(|m| MemoryLinePayload {
                    id: m.stem.to_string(),
                    date: day(m.date),
                    title: m.title.clone(),
                })
                .collect(),
            text: scribe_core::memory::block(view.profile.as_ref(), &view.now, &view.milestones),
            warnings,
        }
    }
}

/// `memory rules`: the fixed memory rules (2_design.md §15.5). Field names
/// are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRulesPayload {
    /// In a fixed order.
    pub(crate) rules: Vec<MemoryRulePayload>,
}

/// One rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryRulePayload {
    /// Its name.
    pub(crate) rule: &'static str,
    /// Its value: a number or the list of phrases.
    pub(crate) value: serde_json::Value,
    /// Where it came from: always shipped, as memory has no switches.
    pub(crate) origin: OriginPayload,
    /// What it does, in one line.
    pub(crate) meaning: &'static str,
}

impl MemoryRulesPayload {
    /// The fixed rules.
    pub(crate) fn new() -> Self {
        use scribe_core::memory as m;
        let rule = |rule, value, meaning| MemoryRulePayload {
            rule,
            value,
            origin: OriginPayload::SHIPPED,
            meaning,
        };
        Self {
            rules: vec![
                rule(
                    "max_per_pass",
                    serde_json::Value::from(m::MAX_PER_PASS),
                    "the most now and milestone elements one pass invocation writes per session",
                ),
                rule(
                    "max_pending",
                    serde_json::Value::from(m::MAX_PENDING),
                    "the most undated now candidates a session holds until a later turn dates one; dropped when the session ends",
                ),
                rule(
                    "now_keep_days",
                    serde_json::Value::from(m::NOW_KEEP_DAYS),
                    "a now item stays this many days after its date, then the weekly refresh makes it a milestone or history",
                ),
                rule(
                    "milestone_lines",
                    serde_json::Value::from(m::MILESTONE_LINES),
                    "the memory block shows this many milestones, newest first",
                ),
                rule(
                    "profile_chars",
                    serde_json::Value::from(m::profile::PROFILE_CHARS),
                    "the longest profile the weekly refresh writes",
                ),
                rule(
                    "user_says",
                    serde_json::Value::from(
                        m::USER_SAYS
                            .iter()
                            .map(|p| (*p).to_owned())
                            .collect::<Vec<_>>(),
                    ),
                    "one of these phrases in the user's turn makes the event a milestone",
                ),
            ],
        }
    }
}

/// `memory refresh`: what the weekly profile job did. Field names are a
/// contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RefreshPayload {
    /// `written`, or `nothing_new` when nothing changed since the last run
    /// and no model was called.
    pub(crate) result: &'static str,
    /// The new profile revision; `null` when the profile did not change.
    pub(crate) revision: Option<i64>,
    /// Sessions read.
    pub(crate) sessions: usize,
    /// Sessions after the cursor left for the next run.
    pub(crate) backlog: usize,
    /// Milestones written, by stem.
    pub(crate) milestones: Vec<String>,
    /// Passed `now` items removed, by stem.
    pub(crate) removed: Vec<String>,
}

/// `store migrate`: the archives imported into `scribe.db`. Field names
/// are a contract. Since phase 8 nothing moves from files, so the counts
/// of moved records (`sessions` through `pickups`, and `audit_torn`) are 0
/// and `moved_to` is `null`; they stay for the contract (STD-01 R10).
/// `audit_lines` still counts the spooled lines the write drained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MigratePayload {
    /// Session records imported.
    pub(crate) sessions: usize,
    /// Switches imported.
    pub(crate) switches: usize,
    /// Codex cursors imported.
    pub(crate) cursors: usize,
    /// Where the files went, under the data directory; `null` when there
    /// were none (always since phase 8).
    pub(crate) moved_to: Option<String>,
    /// Entries imported.
    pub(crate) entries: usize,
    /// Claims imported.
    pub(crate) claims: usize,
    /// Agent tags imported.
    pub(crate) tags: usize,
    /// Annotation marks imported.
    pub(crate) marks: usize,
    /// Pickups imported.
    pub(crate) pickups: usize,
    /// Spooled audit lines drained into the table.
    pub(crate) audit_lines: usize,
    /// Audit lines that did not parse (always 0 since phase 8).
    pub(crate) audit_torn: usize,
    /// `pre-sqlite-*` and `pre-issue-*` directories imported into the
    /// archive tables, then removed.
    pub(crate) archives: usize,
    /// Archived records imported: one per file, one per audit line.
    pub(crate) archived: usize,
}

/// `export`: the files written. Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ExportPayload {
    /// The directory written into.
    pub(crate) dir: String,
    /// `markdown` or `jsonl`.
    pub(crate) format: &'static str,
    /// Sessions written.
    pub(crate) sessions: usize,
    /// Entries written.
    pub(crate) entries: usize,
    /// Claims written.
    pub(crate) claims: usize,
    /// Audit lines written.
    pub(crate) audit_lines: usize,
    /// Every file written, relative to `dir`, in order.
    pub(crate) files: Vec<String>,
    /// Records that did not read, so were not written.
    pub(crate) skipped: Vec<String>,
}

/// `memory show --revision <n>`: one profile revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct RevisionPayload {
    /// Its number.
    pub(crate) revision: i64,
    /// When it was committed.
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) created: Timestamp,
    /// The profile's text.
    pub(crate) text: String,
}

/// `scribe brief`: the last sessions and the open claims
/// (docs/design/scribe/specs/output-contract.md). Field names are a contract.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefPayload {
    /// The memory block: `{profile, now, milestones}`.
    pub(crate) memory: MemoryBlockPayload,
    /// Warnings about the memory block (`memory_pending`, a read-only
    /// copy), each also a stderr line.
    pub(crate) warnings: Vec<String>,
    /// The project whose sessions come first; `null` for none.
    pub(crate) project: Option<String>,
    /// The project's most recently opened group, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) last_group: Option<scribe_core::brief::LastGroup>,
    /// The last sessions: the project's first, each part newest first.
    pub(crate) sessions: Vec<BriefSessionPayload>,
    /// How many open claims `claims` holds.
    pub(crate) claim_count: usize,
    /// The open claims, grouped by tag.
    pub(crate) claims: Vec<BriefClaimGroupPayload>,
    /// The brief as text, exactly as it prints and as the hook injects it.
    #[serde(skip)]
    pub(crate) text: String,
}

/// One session in the brief.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefSessionPayload {
    pub(crate) id: String,
    pub(crate) project: String,
    pub(crate) title: String,
    /// Its last recorded activity.
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) at: Timestamp,
    /// Whether scribe holds a session record; `false` for a handoff-only session.
    pub(crate) has_record: bool,
    /// The newest active handoff entry id, or `null` when none.
    pub(crate) handoff: Option<String>,
    /// How many active handoff entries this session has.
    pub(crate) handoffs: usize,
    pub(crate) handoff_mode: Option<scribe_core::handoff::Mode>,
}

/// The open claims under one tag.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefClaimGroupPayload {
    pub(crate) tag: String,
    pub(crate) claims: Vec<BriefClaimPayload>,
}

/// One open claim: an open proposal, settled by its `id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BriefClaimPayload {
    /// `p-<n>`.
    pub(crate) id: String,
    /// `candidate`, `supersedes` or `stale`.
    pub(crate) kind: &'static str,
    /// The words its line quotes: a candidate's verbatim words, or a
    /// consistency proposal's first quoted span (its reason when it has none).
    pub(crate) quote: String,
    /// A candidate's statement (its quote when it has none), or a
    /// consistency proposal's reason.
    pub(crate) statement: String,
    pub(crate) subject: Option<String>,
    /// The entry it would supersede or retire; `null` for a candidate.
    pub(crate) older: Option<String>,
    pub(crate) newer: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub(crate) opened_at: Timestamp,
}

impl BriefPayload {
    /// The payload of `b`, whose memory block is `memory`.
    pub(crate) fn new(b: &scribe_core::brief::Brief, memory: MemoryBlockPayload) -> Self {
        Self {
            warnings: memory.warnings.clone(),
            memory,
            project: b.project.as_ref().map(ToString::to_string),
            last_group: b.last_group.clone(),
            sessions: b
                .sessions
                .iter()
                .map(|s| BriefSessionPayload {
                    id: s.id.to_string(),
                    project: s.project.to_string(),
                    title: s.title.clone(),
                    at: s.at,
                    has_record: s.has_record,
                    handoff: s.handoff.as_ref().map(ToString::to_string),
                    handoffs: s.handoffs,
                    handoff_mode: s.handoff_mode,
                })
                .collect(),
            claim_count: b.claim_count(),
            claims: b
                .claims
                .iter()
                .map(|g| BriefClaimGroupPayload {
                    tag: g.tag.to_string(),
                    claims: g
                        .claims
                        .iter()
                        .map(|c| BriefClaimPayload {
                            id: c.id.to_string(),
                            kind: c.kind.as_str(),
                            quote: c.quote.clone(),
                            statement: c.statement.clone(),
                            subject: c.subject.clone(),
                            older: c.older.as_ref().map(ToString::to_string),
                            newer: c.newer.as_ref().map(ToString::to_string),
                            opened_at: c.opened_at,
                        })
                        .collect(),
                })
                .collect(),
            text: b.text(),
        }
    }
}

/// One uncataloged transcript (`transcript list` / `transcript show`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TranscriptPayload {
    /// The transcript's id.
    pub(crate) session: String,
    /// The attributed project slug.
    pub(crate) project: String,
    /// Mentions of each configured project's paths. Every configured project
    /// is present.
    pub(crate) project_mentions: BTreeMap<String, u32>,
    /// The transcript's cwd, empty when it names none.
    pub(crate) cwd: String,
    /// The first kept message. `null` when the file has none.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) started: Option<Timestamp>,
    /// The last kept message.
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) ended: Option<Timestamp>,
    /// The file's length.
    pub(crate) bytes: u64,
    /// The user's turns, by the pass's rule.
    pub(crate) user_turns: u32,
    /// Their first turn, cut to 300 characters. Empty when they have none.
    pub(crate) first_prompt: String,
    /// His first five turns, each cut to 300 characters. Present on
    /// `transcript show` only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) user_first_turns: Option<Vec<String>>,
}

/// The reply a Claude Code hook prints: text for the user
/// (`systemMessage`) and for the main agent (`additionalContext`), plus what
/// the audit log records for the run. Its JSON shape is Claude Code's hook
/// protocol, so it is the same in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookReply {
    /// The hook event, such as `UserPromptSubmit`.
    pub(crate) event: &'static str,
    /// Shown to the user; `None` prints no reply at all.
    pub(crate) message: Option<String>,
    /// Shown to the main agent beside the prompt.
    pub(crate) context: Option<String>,
    /// Block the prompt with this reason, shown to the user: a `/scribe`
    /// command is answered here and never reaches the model.
    pub(crate) block: Option<String>,
    /// One per record written or refused, and one per run that stored a group
    /// member's new turns, for the audit log; empty when the hook changed
    /// nothing.
    pub(crate) outcomes: Vec<HookOutcome>,
}

/// One write or refusal a hook run made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HookOutcome {
    /// The record it acted on: an entry id, or `session:<id>` for a refusal.
    pub(crate) target: Option<String>,
    /// The stable code of a refusal; `None` for a write.
    pub(crate) error_code: Option<&'static str>,
}

/// The JSON a hook prints (Claude Code's hook output).
#[derive(Serialize)]
pub(crate) struct HookJson<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) decision: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<&'a str>,
    #[serde(rename = "systemMessage", skip_serializing_if = "Option::is_none")]
    pub(crate) system_message: Option<&'a str>,
    #[serde(rename = "hookSpecificOutput", skip_serializing_if = "Option::is_none")]
    pub(crate) hook_specific_output: Option<HookSpecificJson<'a>>,
}

/// `hookSpecificOutput`.
#[derive(Serialize)]
pub(crate) struct HookSpecificJson<'a> {
    #[serde(rename = "hookEventName")]
    pub(crate) hook_event_name: &'a str,
    #[serde(rename = "additionalContext")]
    pub(crate) additional_context: &'a str,
}

pub(crate) use scribe_core::cases::pass::{PassPayload, PassReport};

/// The run's error of a [`PassReport`].
pub(crate) trait PassFailure {
    /// The run's error when any pass failed, naming each failed session,
    /// code and short reason; `None` when none did.
    fn failure(&self) -> Option<crate::error::CliError>;
}

impl PassFailure for PassReport {
    fn failure(&self) -> Option<crate::error::CliError> {
        let failed: Vec<String> = self
            .passes
            .iter()
            .filter_map(|p| {
                let code = p.error_code?;
                let reason = p.error.as_deref().unwrap_or("reason unavailable");
                Some(format!("{} {code}: {reason}", p.session))
            })
            .collect();
        (!failed.is_empty()).then(|| {
            if self.catalog {
                return crate::error::CliError::PassFailed(format!(
                    "{} session catalog(s) failed: {}; checkpoint kept; repeat 'scribe session catalog <id> --author <name>' to resume",
                    failed.len(),
                    failed.join(", ")
                ));
            }
            if self.passes.iter().any(|p| p.author.is_some()) {
                return crate::error::CliError::PassFailed(format!("{} session rebuild(s) failed: {}; checkpoint kept; repeat 'scribe session rebuild <id> --author <name> --confirm' to resume", failed.len(), failed.join(", ")));
            }
            crate::error::CliError::PassFailed(format!(
                "{} session pass(es) failed: {}; each stays due with retry backoff; use 'scribe session pass --session <id>' to retry now",
                failed.len(),
                failed.join(", ")
            ))
        })
    }
}

/// An approved candidate a scan closed because an active entry already
/// covers its statement. The proposal is `rejected`; no entry was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct CoveredPayload {
    /// The candidate proposal id.
    pub(crate) proposal: String,
    /// The active entry that covers the statement.
    pub(crate) by: String,
}

/// An approved proposal a scan applied again, because its older entry was
/// still `active`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ReappliedPayload {
    /// The proposal id.
    pub(crate) proposal: String,
    /// The older entry's id.
    pub(crate) entry: String,
    /// Its status now: `superseded` or `retired`.
    pub(crate) status: &'static str,
}

/// A batch of pairs the consistency model did not label.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct BatchFailure {
    /// The batch's number in this scan, from 1.
    pub(crate) batch: usize,
    /// The model failure's stable code.
    pub(crate) error_code: &'static str,
}

/// A session whose automatic deactivation failed after the scan found it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeactivationFailure {
    /// The session id.
    pub(crate) session: String,
    /// `age` or `removed`.
    pub(crate) route: &'static str,
    /// The stable storage error code.
    pub(crate) error_code: &'static str,
}

/// What `consistency scan` did; `--json` and the `scribe.consistency-scan`
/// tool print it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ScanReport {
    /// Approvals applied again, by proposal id.
    pub(crate) reapplied: Vec<ReappliedPayload>,
    /// Proposals this scan opened, by id.
    pub(crate) opened: Vec<ProposalPayload>,
    /// Open candidate proposals this scan closed automatically (`tag_retired`).
    pub(crate) closed: Vec<ProposalPayload>,
    /// Approved candidates this scan closed because an active entry already
    /// covers the statement. Not stored on the proposal.
    pub(crate) covered: Vec<CoveredPayload>,
    /// Candidate pairs the model labelled.
    pub(crate) judged_pairs: usize,
    /// Candidate pairs left for a later scan: past the per-scan cap, or not
    /// reached in this call's time.
    pub(crate) pairs_left: usize,
    /// Pairs labelled `unrelated`.
    pub(crate) unrelated_pairs: usize,
    /// Labels dropped: a quote not verbatim in its entry, an unknown pair or
    /// label.
    pub(crate) dropped_labels: usize,
    /// Stale entries found (each opens a proposal unless one exists).
    pub(crate) stale_entries: usize,
    /// Batches the model failed; their pairs are offered again next scan.
    pub(crate) failed_batches: Vec<BatchFailure>,
    /// Sessions this scan deactivated (S10), by route.
    pub(crate) deactivated: Vec<DeactivatedPayload>,
    /// Sessions the scan could not deactivate; other sessions are still
    /// processed.
    pub(crate) failed_deactivations: Vec<DeactivationFailure>,
    /// Removed-feature pairs the model labelled.
    pub(crate) removal_pairs_judged: usize,
    /// Removed-feature pairs past the cap of 20 or not reached in this
    /// call's time.
    pub(crate) removal_pairs_left: usize,
    /// Removed-feature labels refused: a quote not verbatim in the later
    /// record, an unknown or repeated pair, an unknown label.
    pub(crate) dropped_removal_labels: usize,
    /// Removed-feature batches the model failed; their pairs are offered
    /// again while their later record is recent.
    pub(crate) failed_removal_batches: Vec<BatchFailure>,
    /// Always empty since the fixed memory shape (2_design.md §15.5): the
    /// scan no longer consolidates memory. Kept because `--json` field names
    /// are a contract.
    pub(crate) memory: MemoryScanPayload,
    /// Lines for stderr, printed in every mode.
    #[serde(skip)]
    pub(crate) notices: Vec<String>,
}

/// What a scan's memory steps did.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct MemoryScanPayload {
    /// Memory pairs the model labelled.
    pub(crate) pairs_judged: usize,
    /// Memory pairs past the cap of 40 or not reached in this call's time.
    pub(crate) pairs_left: usize,
    /// Pairs labelled `unrelated`, never offered again.
    pub(crate) unrelated: usize,
    /// Labels refused: a quote not verbatim in its item's quotes, an
    /// unknown or repeated pair, an unknown label.
    pub(crate) dropped_labels: usize,
    /// Batches the model failed; their pairs are offered again next scan.
    pub(crate) failed_batches: Vec<BatchFailure>,
    /// Items merged into an older one (`same`).
    pub(crate) merged: Vec<MergedPayload>,
    /// Items a newer one superseded (`contradicts`).
    pub(crate) superseded: Vec<SupersededPayload>,
    /// `now` items promoted to `milestones` (rule (b) after the merges).
    pub(crate) promoted: Vec<PromotedPayload>,
    /// Items changed to complete a promotion a crash cut short.
    pub(crate) completed: Vec<String>,
    /// Every item whose file the scan changed, `memory:<id>`: one audit
    /// line each.
    #[serde(skip)]
    pub(crate) targets: Vec<String>,
}

/// An item a scan merged.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct MergedPayload {
    /// The newer item, now `merged`.
    pub(crate) item: String,
    /// The older item that holds its occurrences.
    pub(crate) into: String,
}

/// An item a scan superseded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct SupersededPayload {
    /// The older item, now `superseded`.
    pub(crate) item: String,
    /// The newer item.
    pub(crate) by: String,
}

/// A promotion.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PromotedPayload {
    /// The `now` item, now `promoted`.
    pub(crate) item: String,
    /// The milestone.
    pub(crate) to: String,
    /// `profile_change`, `recurs_in_sessions` or `user_says`.
    pub(crate) rule: &'static str,
}

/// A session a scan deactivated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct DeactivatedPayload {
    /// The session id.
    pub(crate) session: String,
    /// `age` or `removed`.
    pub(crate) route: &'static str,
    /// Why: scribe's sentence for `age`, the model's for `removed`.
    pub(crate) reason: String,
    /// The later record a `removed` deactivation rests on; `null` for `age`.
    pub(crate) cite: Option<String>,
    /// The later record's verbatim words a `removed` deactivation rests
    /// on; `null` for `age`.
    pub(crate) quote: Option<String>,
}

/// A claim an answer rests on: verbatim-checked against its cite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ClaimPayload {
    /// The statement (model text).
    pub(crate) text: String,
    /// Words verbatim from the cited record.
    pub(crate) quote: String,
    /// An entry id, `session:<id>` or `task:<id>`.
    pub(crate) cite: String,
}

/// Two or more records that disagree.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct ConflictPayload {
    pub(crate) cites: Vec<String>,
    /// Model text.
    pub(crate) note: String,
}

/// An Orbit task a lookup returned.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct TaskPayload {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) tags: Vec<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub(crate) updated: Option<Timestamp>,
    /// Verbatim; empty when the task has none.
    pub(crate) execution_summary: String,
}

impl From<&Task> for TaskPayload {
    fn from(t: &Task) -> Self {
        Self {
            id: t.id.clone(),
            title: t.title.clone(),
            status: t.status.clone(),
            tags: t.tags.clone(),
            updated: t.updated,
            execution_summary: t.execution_summary.clone(),
        }
    }
}

/// One stored record a lookup returned, as stored: exactly one of `entry`,
/// `session` and `task` is set, as `kind` says.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct FoundPayload {
    /// `entry`, `session` or `task`.
    pub(crate) kind: &'static str,
    /// How an answer cites it.
    pub(crate) cite: String,
    pub(crate) entry: Option<EntryPayload>,
    /// The same shape `session show --json` prints.
    pub(crate) session: Option<SessionPayload>,
    pub(crate) task: Option<TaskPayload>,
    /// For a term lookup, the session's terms that matched, with the user's
    /// defining words; empty otherwise.
    pub(crate) terms: Vec<TermPayload>,
}

impl FoundPayload {
    fn new(found: &Found, sessions: &SessionsContext<'_>) -> Self {
        let cite = found.cite();
        match found {
            Found::Entry(entry) => Self {
                kind: "entry",
                cite,
                entry: Some(EntryPayload::from(&**entry)),
                session: None,
                task: None,
                terms: Vec::new(),
            },
            Found::Session { record, terms } => Self {
                kind: "session",
                cite,
                entry: None,
                session: Some(
                    SessionPayload::new(
                        record,
                        sessions.control.get(&record.id),
                        Kin::of(&record.id, sessions.children, sessions.titles),
                    )
                    .with_number(sessions.numbers.get(&record.id).copied())
                    .with_forgotten(sessions.forgotten.get(&record.id).copied())
                    .with_log(&record.log)
                    .with_handoffs(scribe_core::handoff::for_session(
                        sessions.entries,
                        &record.id,
                    )),
                ),
                task: None,
                terms: terms.iter().map(TermPayload::from).collect(),
            },
            Found::Task(task) => Self {
                kind: "task",
                cite,
                entry: None,
                session: None,
                task: Some(TaskPayload::from(task)),
                terms: Vec::new(),
            },
        }
    }
}

/// What a found session record is shown with: every switch, the derived
/// children and every record's title.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SessionsContext<'a> {
    pub(crate) control: &'a ControlState,
    pub(crate) children: &'a BTreeMap<SessionId, Vec<Child>>,
    pub(crate) titles: &'a BTreeMap<SessionId, String>,
    pub(crate) numbers: &'a BTreeMap<SessionId, std::num::NonZeroU64>,
    pub(crate) forgotten: &'a BTreeMap<SessionId, Timestamp>,
    pub(crate) entries: &'a [Entry],
}

/// What `question ask` and the `scribe.ask` tool return (2_design.md §6.1):
/// the same payload from both.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AskPayload {
    /// `answered`, `no_record`, `conflict` or `record` (a lookup).
    pub(crate) status: &'static str,
    /// Model text for an answer; a fixed sentence otherwise.
    pub(crate) answer: String,
    pub(crate) claims: Vec<ClaimPayload>,
    pub(crate) conflicts: Vec<ConflictPayload>,
    /// Evidence items the model was given; records returned, for a lookup.
    pub(crate) evidence_considered: usize,
    /// Claims dropped because their quote or cite failed the check.
    pub(crate) dropped_quotes: usize,
    /// Superseded or retired entries that matched and were left out.
    pub(crate) hidden_matches: usize,
    /// The lookup that answered, or `null` when retrieval ran.
    pub(crate) lookup: Option<&'static str>,
    /// The stored records a lookup returned; empty otherwise.
    pub(crate) records: Vec<FoundPayload>,
    /// Record files left out because they did not parse.
    pub(crate) warnings: Vec<WarningPayload>,
}

impl AskPayload {
    pub(crate) fn new(
        answer: &Answer,
        sessions: &SessionsContext<'_>,
        skipped: &[Skipped],
    ) -> Self {
        Self {
            status: answer.status.as_str(),
            answer: answer.answer.clone(),
            claims: answer
                .claims
                .iter()
                .map(|c| ClaimPayload {
                    text: c.text.clone(),
                    quote: c.quote.clone(),
                    cite: c.cite.clone(),
                })
                .collect(),
            conflicts: answer
                .conflicts
                .iter()
                .map(|c| ConflictPayload {
                    cites: c.cites.clone(),
                    note: c.note.clone(),
                })
                .collect(),
            evidence_considered: answer.evidence_considered,
            dropped_quotes: answer.dropped_quotes,
            hidden_matches: answer.hidden_matches,
            lookup: answer.lookup.map(|l| l.as_str()),
            records: answer
                .records
                .iter()
                .map(|f| FoundPayload::new(f, sessions))
                .collect(),
            warnings: skipped.iter().map(WarningPayload::from).collect(),
        }
    }
}

/// A plugin tool call's reply: Orbit's exec-backend envelope, one line on
/// stdout whatever happened (`{"ok":true,"output":...}` or
/// `{"ok":false,"error":{...}}`), in every output mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PluginReply {
    /// `scribe.ask` answered.
    Ask(Box<AskPayload>),
    /// `scribe.session-pass` ran.
    Pass(PassReport),
    /// `scribe.consistency-scan` ran.
    Scan(Box<ScanReport>),
    /// `scribe.memory-refresh` ran.
    Refresh(Box<RefreshPayload>),
    /// The call failed; the envelope carries the code.
    Error(PluginError),
}

/// A failed plugin call, as the envelope's `error` object.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct PluginError {
    /// A stable `snake_case` code.
    pub(crate) code: &'static str,
    pub(crate) message: String,
    /// Whether the same call may succeed later.
    pub(crate) retryable: bool,
}

impl Output {
    /// A direct write's outcome. An unchanged write says `nothing changed`
    /// on stderr and is not audited.
    pub(crate) fn written(record: Record, changed: bool) -> Self {
        let notices = if changed {
            Vec::new()
        } else {
            vec![String::from("nothing changed")]
        };
        Self::Written {
            record,
            changed,
            notices,
        }
    }
}

/// The rows of one popup list view: sessions, entries, proposals or
/// transcripts, or one notice row when the list is empty or failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PopupRows {
    /// The record kind every row carries: `session`, `entry`, `proposal` or
    /// `transcript`.
    pub(crate) kind: &'static str,
    /// The records to list; `None` when `notice` stands in for them.
    pub(crate) records: Option<Records>,
    /// The one row shown instead of records (an empty or failed list).
    pub(crate) notice: Option<String>,
    /// The current year, so a date of another year shows its year.
    pub(crate) year: i32,
    /// Whether the popup's terminal wants color (still off under `NO_COLOR`
    /// and `TERM=dumb`).
    pub(crate) color: bool,
}

/// Result of backfilling the current holders of a group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GroupBackfill {
    pub(crate) group: String,
    pub(crate) members: Vec<GroupBackfillMember>,
    pub(crate) stored: usize,
    pub(crate) skipped: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GroupBackfillMember {
    pub(crate) seat: String,
    pub(crate) session: String,
    pub(crate) stored: usize,
    pub(crate) skipped: usize,
    pub(crate) transcript_unavailable: bool,
}

/// What a command produced, before rendering.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Output {
    /// Provider integration scaffold result.
    Init(scribe_core::InitReport),
    /// A list result.
    List {
        /// The records returned, in order.
        records: Records,
        /// How many matched before the limit; more than the records when the
        /// list was cut.
        total: usize,
        /// Files left out because they did not parse; each is also a stderr
        /// line.
        warnings: Vec<WarningPayload>,
        /// Lines for stderr, printed in every mode (STD-01 R12): what was
        /// searched when nothing matched (R16), or that the list was cut
        /// (R34).
        notices: Vec<String>,
        /// The stored messages a default `search` found when no entry or
        /// session matched (`message_matches` in `--json`); `None` otherwise,
        /// and then the key is absent.
        message_matches: Option<usize>,
    },
    /// A single record (`show`).
    Show {
        /// The record.
        record: Record,
    },
    /// The record a direct write verb wrote, in the `show` shape (`entry`,
    /// `tag` and `proposal` writes; `session activate`, `session deactivate`).
    Written {
        /// The record as it now stands.
        record: Record,
        /// Whether the write changed it; an unchanged write is not audited.
        changed: bool,
        /// Lines for stderr, printed in every mode: `nothing changed`, or
        /// that a deactivation unpinned the session.
        notices: Vec<String>,
    },
    /// Group backfill counts.
    GroupBackfill(GroupBackfill),
    /// A hook's reply.
    Hook(HookReply),
    /// A session pass's report.
    Pass(PassReport),
    /// A consistency scan's report.
    Scan(Box<ScanReport>),
    /// An answer to a question.
    Ask(Box<AskPayload>),
    /// A plugin tool call's envelope.
    Plugin(PluginReply),
    /// `tmux` ran its popup, which owned the terminal; nothing to render.
    Popup,
    /// The rows of one popup list view (the popup's hidden `tmux rows`).
    PopupRows(PopupRows),
    /// The popup's preview of one record (the hidden `tmux preview`).
    PopupPreview(Record),
}
