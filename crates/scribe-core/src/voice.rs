//! Who prompts name and what each model use is told to do: the
//! configurable half of every prompt (`[user]` and `[instructions]` in
//! `config.toml`).
//!
//! A prompt is the use's instruction text followed by its output contract.
//! The instruction text says what to do and what to catch; `config.toml`
//! may replace it per use. The contract (the answer's JSON shape, the
//! verbatim-quote rule, what is never kept, messages being data) is fixed
//! here in code and always follows. Both halves name the person whose
//! sessions scribe records as "the user", or by `[user] name` when the file
//! sets one: `{user}` and `{User}` in any text stand for that noun, the
//! second capitalised to open a sentence.
//!
//! No I/O (STD-02 R5).

/// The most characters `[user] name` may hold.
pub const USER_NAME_CHARS: usize = 64;

/// The most characters one use's instruction text may hold.
pub const INSTRUCTION_CHARS: usize = 4_000;

/// How prompts name the user: "the user" by default, or `[user] name`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UserName(Option<String>);

impl UserName {
    /// A configured name, checked: nonempty after trimming, at most
    /// [`USER_NAME_CHARS`] characters, no control characters and no braces
    /// (which would read as a placeholder).
    ///
    /// # Errors
    ///
    /// Why the name was refused.
    pub fn named(name: &str) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err(String::from("user.name must not be empty"));
        }
        if name.chars().count() > USER_NAME_CHARS {
            return Err(format!(
                "user.name must be at most {USER_NAME_CHARS} characters"
            ));
        }
        if name.chars().any(|c| c.is_control() || c == '{' || c == '}') {
            return Err(String::from(
                "user.name must contain no control characters or braces",
            ));
        }
        Ok(Self(Some(name.to_owned())))
    }

    /// The configured name, when there is one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.0.as_deref()
    }

    /// The noun inside a sentence: `the user`, or the name.
    #[must_use]
    pub fn noun(&self) -> &str {
        self.0.as_deref().unwrap_or("the user")
    }

    /// The noun opening a sentence: `The user`, or the name.
    #[must_use]
    pub fn title(&self) -> &str {
        self.0.as_deref().unwrap_or("The user")
    }

    /// `text` with `{User}` and `{user}` replaced by [`Self::title`] and
    /// [`Self::noun`]. Other braces are left as they are.
    #[must_use]
    pub fn fill(&self, text: &str) -> String {
        text.replace("{User}", self.title())
            .replace("{user}", self.noun())
    }
}

/// One model use whose instruction text `config.toml` may replace, keyed
/// by its name under `[instructions]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelUse {
    /// Cataloging a `#scribe` annotation.
    Annotation,
    /// The session pass that writes a session record.
    Session,
    /// Shortening a refused session header.
    SessionHeader,
    /// `ask`'s answer from numbered evidence.
    Ask,
    /// The consistency routine's judgement of entry pairs.
    Consistency,
    /// The consistency routine's judgement of memory pairs.
    MemoryPairs,
    /// The consistency routine's judgement of removed-feature pairs.
    Removal,
}

impl ModelUse {
    /// Every use, in the order `config.toml` documents them.
    pub const ALL: [Self; 7] = [
        Self::Annotation,
        Self::Session,
        Self::SessionHeader,
        Self::Ask,
        Self::Consistency,
        Self::MemoryPairs,
        Self::Removal,
    ];

    /// Its key under `[instructions]`.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Self::Annotation => "annotation",
            Self::Session => "session",
            Self::SessionHeader => "session_header",
            Self::Ask => "ask",
            Self::Consistency => "consistency",
            Self::MemoryPairs => "memory_pairs",
            Self::Removal => "removal",
        }
    }
}

/// The instruction texts `[instructions]` sets, per use; a use it leaves
/// out keeps its shipped text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Instructions {
    texts: Vec<(ModelUse, String)>,
}

impl Instructions {
    /// Set `model_use`'s text, checked: nonempty after trimming, at most
    /// [`INSTRUCTION_CHARS`] characters, and no control characters other
    /// than newlines and tabs.
    ///
    /// # Errors
    ///
    /// Why the text was refused.
    pub fn set(&mut self, model_use: ModelUse, text: &str) -> Result<(), String> {
        let key = model_use.key();
        let text = text.trim();
        if text.is_empty() {
            return Err(format!("instructions.{key} must not be empty"));
        }
        if text.chars().count() > INSTRUCTION_CHARS {
            return Err(format!(
                "instructions.{key} must be at most {INSTRUCTION_CHARS} characters"
            ));
        }
        if text
            .chars()
            .any(|c| c.is_control() && c != '\n' && c != '\t')
        {
            return Err(format!(
                "instructions.{key} must contain no control characters other than newlines and tabs"
            ));
        }
        self.texts.retain(|(u, _)| *u != model_use);
        self.texts.push((model_use, text.to_owned()));
        Ok(())
    }

    /// The text `config.toml` set for `model_use`, when it set one.
    #[must_use]
    pub fn get(&self, model_use: ModelUse) -> Option<&str> {
        self.texts
            .iter()
            .find(|(u, _)| *u == model_use)
            .map(|(_, text)| text.as_str())
    }
}

/// What one request's prompt is voiced with: who it names and, when
/// `config.toml` replaced it, the use's instruction text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    /// How the prompt names the user.
    pub user: UserName,
    /// The configured instruction text; `None` keeps the shipped one.
    pub instructions: Option<String>,
}

impl Voice {
    /// The instruction text to send: the configured one, or `shipped`, with
    /// the user's name filled in.
    #[must_use]
    pub fn instructions(&self, shipped: &str) -> String {
        self.user
            .fill(self.instructions.as_deref().unwrap_or(shipped))
    }

    /// Fixed contract text with the user's name filled in.
    #[must_use]
    pub fn fill(&self, text: &str) -> String {
        self.user.fill(text)
    }
}
