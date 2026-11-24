//! `scribe entry list`. Read-only: no lock, no audit, no write.

/// What `entry list` reads: the filter as typed, its `--related` and
/// `--since`/`--until` still to resolve and check.
pub struct ListInput {
    /// The filter; its `related` and `dates` are replaced.
    pub filter: crate::EntryFilter,
    /// `--since`.
    pub since: Option<time::Date>,
    /// `--until`.
    pub until: Option<time::Date>,
    /// `--group`: a group id, unique prefix or project group slug.
    pub group: Option<String>,
    /// The provider whose transcripts resolve session prefixes.
    pub provider: crate::Provider,
}

/// What `entry list` found.
pub struct Listed {
    /// The entries.
    pub found: crate::EntryList,
    /// The filter as applied.
    pub filter: crate::EntryFilter,
    /// The group it was scoped to.
    pub group: Option<crate::group::Group>,
    /// The action state of each entry.
    pub actions: super::written::Actions,
}

/// `entry list`: resolve `--related` keeping each reference as given, check
/// the dates, then read the group, the entries and their action state.
///
/// # Errors
///
/// [`ReadError`](crate::runtime::errors::ReadError): an ambiguous
/// prefix, a range that ends before it starts, or a store error.
pub fn list<E: crate::runtime::PortError>(
    ctx: &crate::runtime::Ctx<'_, E>,
    input: ListInput,
) -> std::result::Result<Listed, crate::runtime::errors::ReadError<E>> {
    use crate::runtime::errors::ReadError;
    let mut filter = input.filter;
    filter.related =
        crate::runtime::resolve::relateds_preserving_input(ctx, input.provider, &filter.related)?;
    filter.dates = crate::DateRange::checked(input.since, input.until).map_err(ReadError::Usage)?;
    let group = input
        .group
        .as_deref()
        .map(|id| ctx.store.read().groups().get(id))
        .transpose()?;
    let members = group.as_ref().map(crate::group::Group::members);
    let found = ctx
        .store
        .read()
        .entries()
        .list_scoped(&filter, members.as_ref())?;
    let actions = super::written::Actions::load(ctx)?;
    Ok(Listed {
        found,
        filter,
        group,
        actions,
    })
}
