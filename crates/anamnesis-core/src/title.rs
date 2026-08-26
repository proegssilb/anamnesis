//! `Title`: a validated, non-empty, length-bounded string.
//!
//! Parse, don't validate: once you hold a `Title`, it is guaranteed trimmed,
//! non-empty, and at most 200 characters. No downstream layer re-checks it.

use serde::{Deserialize, Serialize};

/// Maximum length, in characters, of a trimmed `Title`.
const MAX_LEN: usize = 200;

/// A trimmed, non-empty string of at most 200 characters.
///
/// The only way to obtain a `Title` is [`Title::new`], which validates and
/// trims. Once constructed, a `Title` is guaranteed valid for the rest of its
/// life; nothing downstream needs to re-check it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Title(String);

/// Why a candidate string could not become a [`Title`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TitleError {
    /// The trimmed input was empty (all whitespace, or literally empty).
    #[error("title must not be empty")]
    Empty,
    /// The trimmed input exceeded 200 characters.
    #[error("title must be at most {MAX_LEN} characters")]
    TooLong,
}

impl Title {
    /// Validates and trims `raw`, returning a `Title` or the reason it was
    /// rejected.
    ///
    /// The 200-character limit is measured against the *trimmed* content.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, TitleError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(TitleError::Empty);
        }
        if trimmed.chars().count() > MAX_LEN {
            return Err(TitleError::TooLong);
        }
        Ok(Self(trimmed.to_string()))
    }

    /// The validated, trimmed title text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Title {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for Title {
    type Error = TitleError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Title::new(value)
    }
}

impl From<Title> for String {
    fn from(title: Title) -> Self {
        title.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case("   ")]
    #[case("")]
    #[case("\t\n  \t")]
    fn rejects_whitespace_only(#[case] raw: &str) {
        assert_eq!(Title::new(raw), Err(TitleError::Empty));
    }

    #[test]
    fn trims_surrounding_whitespace() {
        let title = Title::new("  hello world  ").unwrap();
        assert_eq!(title.as_str(), "hello world");
    }

    #[test]
    fn rejects_201_chars() {
        let s = "a".repeat(201);
        assert_eq!(Title::new(s), Err(TitleError::TooLong));
    }

    #[test]
    fn accepts_200_chars() {
        let s = "a".repeat(200);
        let title = Title::new(s.clone()).unwrap();
        assert_eq!(title.as_str(), s);
    }

    #[test]
    fn accepts_200_chars_after_trimming_surrounding_whitespace() {
        // The limit applies to the trimmed content, not the raw input.
        let s = format!("  {}  ", "a".repeat(200));
        let title = Title::new(s).unwrap();
        assert_eq!(title.as_str().len(), 200);
    }
}
