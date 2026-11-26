//! Observations shared by subject checks. A fixture supplies the isolated
//! command. The observation owns its process lifetime and exit contract.

use super::{ChildGuard, Fixture, text};
use std::io::Write;
use std::process::{Command, Output, Stdio};

pub trait Expectation {
    type Output;

    #[track_caller]
    fn observe(self, command: &mut Command) -> Self::Output;
}

pub enum Exit {
    Success,
    Capture,
    Code(i32),
}

impl Exit {
    pub fn stdin(self, bytes: &[u8]) -> WithStdin<'_> {
        WithStdin { exit: self, bytes }
    }

    #[track_caller]
    fn compare(self, command: &Command, out: Output) -> Output {
        let expected = match self {
            Self::Capture => return out,
            Self::Success => 0,
            Self::Code(code) => code,
        };
        assert_eq!(
            out.status.code(),
            Some(expected),
            "{:?}: {}",
            command.get_args(),
            text(&out.stderr)
        );
        out
    }
}

impl Expectation for Exit {
    type Output = Output;

    #[track_caller]
    fn observe(self, command: &mut Command) -> Output {
        let out = command.output().expect("run command");
        self.compare(command, out)
    }
}

/// Require a successful command and decode its entire stdout as JSON.
pub struct Json;

impl Expectation for Json {
    type Output = serde_json::Value;

    #[track_caller]
    fn observe(self, command: &mut Command) -> Self::Output {
        let out = Exit::Success.observe(command);
        serde_json::from_slice(&out.stdout).expect("JSON on stdout")
    }
}

/// Require a successful command and UTF-8 stdout.
pub struct Text;

impl Expectation for Text {
    type Output = String;

    #[track_caller]
    fn observe(self, command: &mut Command) -> String {
        let out = Exit::Success.observe(command);
        String::from_utf8(out.stdout).expect("UTF-8 on stdout")
    }
}

pub struct WithStdin<'a> {
    exit: Exit,
    bytes: &'a [u8],
}

impl Expectation for WithStdin<'_> {
    type Output = Output;

    #[track_caller]
    fn observe(self, command: &mut Command) -> Output {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn command with piped stdin");
        let mut child = ChildGuard::new(child);
        child
            .child()
            .stdin
            .take()
            .expect("piped stdin")
            .write_all(self.bytes)
            .expect("write command input");
        self.exit.compare(command, child.finish())
    }
}

/// Subject arguments use the target's command prefix and default environment.
/// `Cli` and `with_env` take complete argv. A prepared command can also
/// exercise a shell, a pseudo-terminal, or a packaged launcher.
pub enum Input<'a> {
    Subject(&'a [&'a str]),
    Cli(&'a [&'a str]),
    WithEnv(&'a [&'a str], &'a str, std::ffi::OsString),
    Prepared(&'a mut Command),
}

impl Input<'_> {
    #[track_caller]
    pub fn run<E: Expectation>(self, fixture: &Fixture, prefix: &[&str], expect: E) -> E::Output {
        self.run_with(fixture, prefix, expect, |_, _| {})
    }

    #[track_caller]
    pub fn run_with<E: Expectation>(
        self,
        fixture: &Fixture,
        prefix: &[&str],
        expect: E,
        configure: impl FnOnce(&Fixture, &mut Command),
    ) -> E::Output {
        match self {
            Self::Subject(args) => {
                let mut command = fixture.cmd(prefix);
                command.args(args);
                configure(fixture, &mut command);
                expect.observe(&mut command)
            }
            Self::Cli(args) => expect.observe(&mut fixture.cmd(args)),
            Self::WithEnv(args, key, value) => expect.observe(fixture.cmd(args).env(key, value)),
            Self::Prepared(command) => expect.observe(command),
        }
    }
}

impl<'a, A: ?Sized + AsRef<[&'a str]>> From<&'a A> for Input<'a> {
    fn from(args: &'a A) -> Self {
        Self::Subject(args.as_ref())
    }
}

impl<'a> From<&'a mut Command> for Input<'a> {
    fn from(command: &'a mut Command) -> Self {
        Self::Prepared(command)
    }
}

/// A full CLI invocation with one environment override.
pub fn with_env<'a>(
    args: &'a [&'a str],
    key: &'a str,
    value: impl AsRef<std::ffi::OsStr>,
) -> Input<'a> {
    Input::WithEnv(args, key, value.as_ref().to_owned())
}
