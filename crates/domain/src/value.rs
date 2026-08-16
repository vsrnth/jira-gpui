use std::fmt;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// A UTC instant. Keeping this alias in the domain makes a future time-library
/// migration local to this crate.
pub type Timestamp = OffsetDateTime;

/// A value rejected by a domain constructor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainError {
    Empty { field: &'static str },
    TooLong { field: &'static str, maximum: usize },
    InvalidIssueKey(String),
}

impl fmt::Display for DomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty { field } => write!(f, "{field} must not be empty"),
            Self::TooLong { field, maximum } => {
                write!(f, "{field} must be at most {maximum} characters")
            }
            Self::InvalidIssueKey(value) => write!(f, "{value:?} is not a Jira issue key"),
        }
    }
}

impl std::error::Error for DomainError {}

macro_rules! string_id {
    ($name:ident, $docs:literal) => {
        #[doc = $docs]
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
                let value = value.into();
                if value.trim().is_empty() {
                    return Err(DomainError::Empty {
                        field: stringify!($name),
                    });
                }
                if value.len() > 255 {
                    return Err(DomainError::TooLong {
                        field: stringify!($name),
                        maximum: 255,
                    });
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

string_id!(JiraSiteId, "Atlassian cloud ID for a Jira site.");
string_id!(AccountId, "Stable Atlassian account ID.");
string_id!(IssueId, "Jira's stable issue ID.");
string_id!(UserSetId, "Local stable identifier for a saved user set.");
string_id!(EventId, "Local stable identifier for an update event.");

/// A human-facing Jira issue key, such as `PROJ-123`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct IssueKey(String);

impl IssueKey {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        let (project, number) = value
            .rsplit_once('-')
            .ok_or_else(|| DomainError::InvalidIssueKey(value.clone()))?;
        // Jira project keys are commonly uppercase alphanumeric strings, but
        // installations may also use underscores. Requiring a leading letter
        // preserves a useful guard without rejecting those valid keys.
        let valid_project = project
            .chars()
            .next()
            .is_some_and(|character| character.is_ascii_uppercase())
            && project.chars().all(|character| {
                character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
            });
        let valid_number =
            !number.is_empty() && number.chars().all(|character| character.is_ascii_digit());
        if valid_project && valid_number {
            Ok(Self(value))
        } else {
            Err(DomainError::InvalidIssueKey(value))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IssueKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_keys_require_an_uppercase_project_and_number() {
        assert!(IssueKey::new("APP-42").is_ok());
        assert!(IssueKey::new("APP_CORE-42").is_ok());
        assert!(IssueKey::new("app-42").is_err());
        assert!(IssueKey::new("1APP-42").is_err());
        assert!(IssueKey::new("APP-nope").is_err());
        assert!(IssueKey::new("APP42").is_err());
    }
}
