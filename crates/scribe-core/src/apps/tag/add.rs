//! `scribe tag add` (ORB-27707 step 3d): an agent tag, without changing
//! config.toml.

use super::redactor;
use super::verbs::TagVerbs;
use crate::runtime::Ctx;
use crate::runtime::errors::WriteError;
use crate::{Author, AuthorName, StoreTag, TagName};

/// What `tag add` runs.
pub struct TagAddInput {
    /// The tag's name.
    pub name: TagName,
    /// Its aliases.
    pub aliases: Vec<TagName>,
    /// Its description.
    pub description: String,
    /// Days after which its entries are due for review.
    pub stale_after_days: Option<std::num::NonZeroU32>,
    /// The agent adding it.
    pub author: AuthorName,
}

/// Add an agent tag. Arguments are checked first: a bad or repeated tag is
/// reported before an unset `[redact]` env name.
///
/// # Errors
///
/// [`WriteError`]: a refusal, the redactor, or a store error.
pub fn add<E>(ctx: &Ctx<'_, E>, input: TagAddInput) -> Result<StoreTag, WriteError<E>> {
    let now = ctx.clock.now();
    let tag = StoreTag {
        name: input.name,
        aliases: input.aliases,
        description: input.description,
        stale_after_days: input.stale_after_days,
        author: Author::agent(input.author),
        updated_by: None,
        created: now,
        updated: now,
    };
    crate::check_agent_author(&tag.author.name)?;
    ctx.store.check_new_tag(&tag)?;
    let redactor = redactor(ctx)?;
    Ok(ctx.store.add_agent_tag(tag, &redactor)?)
}
