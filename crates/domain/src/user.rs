use serde::{Deserialize, Serialize};

use crate::{AccountId, JiraSiteId};

/// A Jira user as visible to the authenticated account.
///
/// `account_id` is the identity; display data may change between synchronizations.
const MAX_DISPLAY_NAME_BYTES: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub site_id: JiraSiteId,
    pub account_id: AccountId,
    #[serde(deserialize_with = "deserialize_display_name")]
    pub display_name: String,
    pub avatar_url: Option<String>,
    pub active: bool,
}

impl User {
    pub fn new(
        site_id: JiraSiteId,
        account_id: AccountId,
        display_name: impl Into<String>,
        avatar_url: Option<String>,
        active: bool,
    ) -> Self {
        Self {
            site_id,
            account_id,
            display_name: normalize_display_name(display_name.into()),
            avatar_url,
            active,
        }
    }
}

fn normalize_display_name(value: String) -> String {
    let value = value.trim();
    let mut end = value.len().min(MAX_DISPLAY_NAME_BYTES);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let value = value[..end].to_owned();
    if value.is_empty() {
        "Unknown user".to_owned()
    } else {
        value
    }
}

fn deserialize_display_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(normalize_display_name(String::deserialize(deserializer)?))
}

#[cfg(test)]
mod tests {
    use super::User;
    use crate::{AccountId, JiraSiteId};

    #[test]
    fn normalizes_and_bounds_display_names() {
        let user = User::new(
            JiraSiteId::new("site").expect("site"),
            AccountId::new("account").expect("account"),
            format!("  {}  ", "é".repeat(200)),
            None,
            true,
        );
        assert!(user.display_name.len() <= 255);
        assert!(user.display_name.is_char_boundary(user.display_name.len()));

        let empty = User::new(
            JiraSiteId::new("site").expect("site"),
            AccountId::new("account").expect("account"),
            "   ",
            None,
            true,
        );
        assert_eq!(empty.display_name, "Unknown user");
    }
}
