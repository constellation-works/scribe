#![expect(
    clippy::disallowed_methods,
    reason = "session unit tests stage temporary transcript fixtures on disk"
)]

mod activity;
mod catalog;
mod control;
mod message;
mod pass;
mod sessions;
mod voice;
