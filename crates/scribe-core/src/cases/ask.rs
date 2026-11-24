//! `scribe ask` and the Orbit tool `scribe.ask` (ORB-27707 step 3d): answer
//! a question from the store with verbatim-checked quotes
//! (docs/design/scribe/2_design.md §6). It reads the store once (entries,
//! session records and `control.json`, no lock, no write), then runs
//! [`crate::ask::answer`]: a lookup answers from the stored records with no
//! model call, otherwise retrieval packs evidence for the `ask` model.

use crate::ask::{self, Answer, Corpus, NoTasks, Question, Tasks};
use crate::cases::{Ctx, PortError};
use crate::group::Group;
use crate::models::ModelError;
use crate::store::{EntryList, SessionList};
use crate::{Config, EntryFilter, Models, Provider, SessionFilter};

/// Why `ask` did not answer.
#[derive(Debug)]
pub enum AskError {
    /// A store error.
    Core(crate::Error),
    /// The model call failed.
    Model(ModelError),
}

impl From<crate::Error> for AskError {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

impl From<ModelError> for AskError {
    fn from(err: ModelError) -> Self {
        Self::Model(err)
    }
}

/// What `ask` runs.
pub struct AskInput<'i, F> {
    /// The question and its scope.
    pub question: Question,
    /// Answer only from this group's members and messages (id, prefix or
    /// slug); no task source is asked then.
    pub group: Option<&'i str>,
    /// The provider whose model defaults resolve the config; `None` reads
    /// the config as stored (the Orbit tool).
    pub provider: Option<Provider>,
    /// Builds the `Models` adapter for the config.
    pub models: F,
    /// The task source, unless a group is named.
    pub tasks: &'i dyn Tasks,
}

/// The answer and the records it was drawn from.
#[derive(Debug)]
pub struct Asked {
    /// The answer.
    pub answer: Answer,
    /// The entries read.
    pub entries: EntryList,
    /// The session records read.
    pub sessions: SessionList,
}

/// Answer the question. Tag names in the scope are resolved through the
/// configured aliases.
///
/// # Errors
///
/// [`AskError`]: a store error or a failed model call.
pub fn run<E: PortError, M: Models>(
    ctx: &Ctx<'_, E>,
    input: AskInput<'_, impl FnOnce(&Config) -> M>,
) -> Result<Asked, AskError> {
    let AskInput {
        mut question,
        group,
        provider,
        models,
        tasks,
    } = input;
    let store = ctx.store;
    let now = ctx.clock.now();
    let config = store.read().tags().config()?;
    let config = match provider {
        Some(provider) => config.for_provider(provider),
        None => config,
    };
    let group: Option<Group> = group.map(|id| store.read().groups().get(id)).transpose()?;
    let tasks: &dyn Tasks = if group.is_some() { &NoTasks } else { tasks };
    let members = group.as_ref().map(Group::members);
    let entries = store
        .read()
        .entries()
        .list_scoped(&EntryFilter::default(), members.as_ref())?;
    let sessions = store.read().sessions().list_scoped(
        &SessionFilter {
            include_inactive: group.is_some(),
            ..SessionFilter::default()
        },
        members.as_ref(),
    )?;
    let messages = if let Some(group) = &group {
        store
            .read()
            .groups()
            .search_messages(&group.id, &ask::tokens(&question.text))?
            .matches
    } else {
        Vec::new()
    };
    let mut historical = Vec::new();
    for tag in &mut question.scope.tags {
        if let Some(def) = config.tags.resolve(tag.as_str()) {
            // The new alias also names a historical tag with its own entries.
            if tag.as_str() == "decisions" && def.name.as_str() == "decision" {
                historical.push(tag.clone());
            }
            *tag = def.name.clone();
        }
    }
    question.scope.tags.extend(historical);
    let corpus = Corpus {
        entries: &entries.listing.items,
        sessions: &sessions.listing.items,
        messages: &messages,
        tags: &config.tags,
        user: &config.user,
    };
    let adapter = models(&config);
    let answer = ask::answer(
        &question,
        corpus,
        &adapter,
        tasks,
        &config.models.ask,
        &config.voice(crate::voice::ModelUse::Ask),
        now,
    )?;
    Ok(Asked {
        answer,
        entries,
        sessions,
    })
}
