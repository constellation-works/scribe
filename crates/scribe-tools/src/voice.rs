//! The user noun and configurable instruction text used by model requests.

/// The most characters `[user] name` may hold.
pub const USER_NAME_CHARS: usize = 64;

/// The most characters one use's instruction text may hold.
pub const INSTRUCTION_CHARS: usize = 4_000;

/// How prompts name the user: "the user" by default, or `[user] name`.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "String", into = "String")]
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

impl TryFrom<String> for UserName {
    type Error = String;

    fn try_from(noun: String) -> Result<Self, Self::Error> {
        if noun == "the user" {
            Ok(Self::default())
        } else {
            Self::named(&noun)
        }
    }
}

impl From<UserName> for String {
    fn from(name: UserName) -> Self {
        name.noun().to_owned()
    }
}

/// What one request's prompt is voiced with: who it names and, when
/// `config.toml` replaced it, the use's instruction text.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Voice {
    /// How the prompt names the user.
    #[serde(rename = "user_noun")]
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
