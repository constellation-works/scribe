use crate::support::command::{Expectation, Input};
use std::process::Command;

impl crate::support::Fixture {
    #[track_caller]
    pub(crate) fn check<'a, E: Expectation>(
        &self,
        input: impl Into<Input<'a>>,
        expect: E,
    ) -> E::Output {
        input.into().run(self, &[], expect)
    }
}

/// Read one line and close stdout while scribe is still writing.
pub(crate) struct CloseReader;

impl Expectation for CloseReader {
    type Output = (String, std::process::Output);

    #[track_caller]
    fn observe(self, command: &mut Command) -> Self::Output {
        use crate::support::ChildGuard;
        use std::io::{BufRead, BufReader};
        use std::process::Stdio;
        let mut child = ChildGuard::new(
            command
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("start writer"),
        );
        let mut first = String::new();
        BufReader::new(child.child().stdout.take().expect("piped stdout"))
            .read_line(&mut first)
            .expect("read first line");
        (first, child.finish())
    }
}

#[track_caller]
pub(super) fn environment(
    fx: &crate::support::Fixture,
    args: &[&str],
    changes: &[(&str, Option<&str>)],
) -> std::process::Output {
    let mut cmd = fx.cmd(args);
    for (name, value) in changes {
        if let Some(value) = value {
            cmd.env(name, value);
        } else {
            cmd.env_remove(name);
        }
    }
    fx.check(&mut cmd, crate::support::command::Exit::Capture)
}
