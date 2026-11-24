//! The error of the record-writing use cases (ORB-27707 step 3d): `entry`,
//! `proposal`, `tag` and `session activate|deactivate` writes.

/// Why a record write did not run.
#[derive(Debug)]
pub enum WriteError<E> {
    /// A store error or refusal.
    Core(crate::Error),
    /// A port failed.
    Port(E),
    /// A usage error (exit 2).
    Usage(String),
    /// A transcript could not be read.
    Read {
        /// The file.
        what: String,
        /// The read error.
        source: std::io::Error,
    },
    /// A permanent write without `--confirm`: the record it would change.
    ConfirmRequired(String),
}

impl<E> From<crate::Error> for WriteError<E> {
    fn from(err: crate::Error) -> Self {
        Self::Core(err)
    }
}

/// The redactor for the stored config, as the record writes build it.
pub(crate) fn redactor<E>(
    ctx: &crate::cases::Ctx<'_, E>,
) -> Result<crate::Redactor, WriteError<E>> {
    let config = ctx.store.read().tags().config()?;
    ctx.redaction.redactor(&config).map_err(WriteError::Port)
}
