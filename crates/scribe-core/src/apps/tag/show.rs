//! `scribe tag show`: one tag's settings and capture rules. Read-only: no
//! lock, no audit, no write.

use super::ConfigReads;
use crate::runtime::Ctx;
use crate::{Result, TagDef};

/// One tag's definition, where it came from, and the config file a
/// `config` origin names.
pub struct Shown {
    /// The definition.
    pub def: TagDef,
    /// Where it came from.
    pub source: &'static str,
    /// The config file.
    pub config_file: String,
}

/// The tag `word` names, by name or alias.
///
/// # Errors
///
/// An unknown tag, or a store error.
pub fn show<E>(ctx: &Ctx<'_, E>, word: &str) -> Result<Shown> {
    let snapshot = ctx.store.read();
    let (def, source) = snapshot.tags().definition(word)?;
    let config_file = snapshot.tags().config_path().display().to_string();
    Ok(Shown {
        def,
        source,
        config_file,
    })
}
