//! Transcript and model transport identities. Missing stored identity means
//! Claude Code for compatibility with stores predating Codex support.

use serde::{Deserialize, Serialize};
use std::{fmt, str::FromStr};

/// Supported host transcript and model transports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    /// Claude Code transcripts and the Claude CLI.
    #[default]
    #[serde(rename = "claude-code")]
    ClaudeCode,
    /// Codex desktop transcripts and the Codex CLI.
    #[serde(rename = "codex")]
    Codex,
}

impl Provider {
    /// Stable persisted spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Provider {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "claude-code" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            _ => Err(String::from("provider must be claude-code or codex")),
        }
    }
}
