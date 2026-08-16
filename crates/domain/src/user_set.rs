use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::{AccountId, JiraSiteId, Timestamp, UserSetId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserSetError {
    EmptyName,
    NameTooLong,
    DuplicateMember(AccountId),
}

impl std::fmt::Display for UserSetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyName => f.write_str("a user set needs a name"),
            Self::NameTooLong => f.write_str("a user-set name may not exceed 120 characters"),
            Self::DuplicateMember(member) => write!(f, "{member} appears more than once"),
        }
    }
}

impl std::error::Error for UserSetError {}

/// A locally saved, ordered selection of Jira users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserSet {
    pub id: UserSetId,
    pub site_id: JiraSiteId,
    pub name: String,
    pub members: Vec<AccountId>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl UserSet {
    pub fn new(
        id: UserSetId,
        site_id: JiraSiteId,
        name: impl Into<String>,
        members: Vec<AccountId>,
        now: Timestamp,
    ) -> Result<Self, UserSetError> {
        let mut user_set = Self {
            id,
            site_id,
            name: String::new(),
            members: Vec::new(),
            created_at: now,
            updated_at: now,
        };
        user_set.rename(name, now)?;
        user_set.replace_members(members, now)?;
        Ok(user_set)
    }

    pub fn rename(&mut self, name: impl Into<String>, now: Timestamp) -> Result<(), UserSetError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(UserSetError::EmptyName);
        }
        if name.chars().count() > 120 {
            return Err(UserSetError::NameTooLong);
        }
        self.name = name;
        self.updated_at = now;
        Ok(())
    }

    pub fn replace_members(
        &mut self,
        members: Vec<AccountId>,
        now: Timestamp,
    ) -> Result<(), UserSetError> {
        let mut seen = HashSet::with_capacity(members.len());
        for member in &members {
            if !seen.insert(member) {
                return Err(UserSetError::DuplicateMember(member.clone()));
            }
        }
        self.members = members;
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use time::macros::datetime;

    use super::*;

    fn id(value: &str) -> AccountId {
        AccountId::new(value).unwrap()
    }

    #[test]
    fn user_sets_preserve_member_order_and_reject_duplicates() {
        let now = datetime!(2026-01-01 00:00 UTC);
        let result = UserSet::new(
            UserSetId::new("team").unwrap(),
            JiraSiteId::new("site").unwrap(),
            "Platform",
            vec![id("first"), id("first")],
            now,
        );
        assert!(matches!(result, Err(UserSetError::DuplicateMember(_))));

        let set = UserSet::new(
            UserSetId::new("team").unwrap(),
            JiraSiteId::new("site").unwrap(),
            "Platform",
            vec![id("second"), id("first")],
            now,
        )
        .unwrap();
        assert_eq!(set.members, vec![id("second"), id("first")]);
    }
}
