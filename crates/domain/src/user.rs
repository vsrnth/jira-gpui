use serde::{Deserialize, Serialize};

use crate::{AccountId, JiraSiteId};

/// A Jira user as visible to the authenticated account.
///
/// `account_id` is the identity; display data may change between synchronizations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub site_id: JiraSiteId,
    pub account_id: AccountId,
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
            display_name: display_name.into(),
            avatar_url,
            active,
        }
    }
}
