//! `scribe session list`. Read-only: no lock, no audit, no write.

/// What `session list` reads: the filter as typed, its `--related` and
/// `--since`/`--until` still to resolve and check.
pub struct ListInput {
    /// The filter; its `related` and `dates` are replaced.
    pub filter: crate::SessionFilter,
    /// `--since`.
    pub since: Option<time::Date>,
    /// `--until`.
    pub until: Option<time::Date>,
    /// `--group`: a group id, unique prefix or project group slug.
    pub group: Option<String>,
    /// The provider whose transcripts resolve session prefixes.
    pub provider: crate::Provider,
}

/// What `session list` found.
pub struct Listed {
    /// The sessions.
    pub found: crate::SessionList,
    /// The filter as applied.
    pub filter: crate::SessionFilter,
    /// The group it was scoped to.
    pub group: Option<crate::group::Group>,
}

/// `session list`: resolve `--related` (a session prefix becomes the full
/// id), check the dates, then read the group and the sessions.
///
/// # Errors
///
/// [`ReadError`](crate::runtime::errors::ReadError): an ambiguous
/// prefix, a range that ends before it starts, or a store error.
pub fn list<E: crate::runtime::PortError>(
    ctx: &crate::runtime::Ctx<'_, E>,
    input: ListInput,
) -> std::result::Result<Listed, crate::runtime::errors::ReadError<E>> {
    let mut filter = input.filter;
    filter.related = crate::runtime::resolve::relateds(ctx, input.provider, &filter.related)?;
    filter.dates = crate::DateRange::checked(input.since, input.until)
        .map_err(crate::runtime::errors::ReadError::Usage)?;
    let group = input
        .group
        .as_deref()
        .map(|id| ctx.store.read().groups().get(id))
        .transpose()?;
    let members = group.as_ref().map(crate::group::Group::members);
    let found = super::list_sessions(&ctx.store.read(), &filter, members.as_ref())?;
    Ok(Listed {
        found,
        filter,
        group,
    })
}
