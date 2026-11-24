//! `scribe proposal list`. Read-only: no lock, no audit, no write.

/// What `proposal list` reads: the filter as typed, its `--entry` and
/// `--since`/`--until` still to resolve and check.
pub struct ListInput<'i> {
    /// The filter; its `entry` and `dates` are replaced.
    pub filter: crate::ProposalFilter,
    /// `--entry`: an entry id or unique prefix.
    pub entry: Option<&'i str>,
    /// `--since`.
    pub since: Option<time::Date>,
    /// `--until`.
    pub until: Option<time::Date>,
}

/// `proposal list`: resolve `--entry`, check the dates, then keep the
/// proposals the filter passes. The filter as applied, and the proposals.
///
/// # Errors
///
/// [`ReadError`](crate::runtime::errors::ReadError): an unknown entry, a
/// range that ends before it starts, or a store error.
pub fn list<E>(
    ctx: &crate::runtime::Ctx<'_, E>,
    input: ListInput<'_>,
) -> std::result::Result<
    (crate::ProposalFilter, Vec<crate::Proposal>),
    crate::runtime::errors::ReadError<E>,
> {
    let mut filter = input.filter;
    filter.entry = match input.entry {
        Some(id) => Some(crate::apps::entry::resolve_entry(ctx.store, id)?.id),
        None => None,
    };
    filter.dates = crate::DateRange::checked(input.since, input.until)
        .map_err(crate::runtime::errors::ReadError::Usage)?;
    let proposals = ctx
        .store
        .read()
        .proposals()
        .all()?
        .into_iter()
        .filter(|p| filter.matches(p))
        .collect();
    Ok((filter, proposals))
}
