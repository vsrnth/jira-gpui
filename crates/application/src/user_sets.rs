use std::collections::HashSet;
use std::sync::Arc;

use jira_domain::{JiraSiteId, UserSet, UserSetId};

use crate::{ApplicationError, UserSetDraft, UserSetPort};

#[derive(Clone)]
pub struct UserSetService {
    repository: Arc<dyn UserSetPort>,
}

impl UserSetService {
    pub fn new(repository: Arc<dyn UserSetPort>) -> Self {
        Self { repository }
    }

    pub async fn list(&self, site_id: &JiraSiteId) -> Result<Vec<UserSet>, ApplicationError> {
        self.repository.list(site_id).await
    }

    pub async fn save(&self, draft: UserSetDraft) -> Result<UserSet, ApplicationError> {
        if draft.name.trim().is_empty() {
            return Err(ApplicationError::invalid_input(
                "user set name cannot be empty",
            ));
        }
        if draft.members.is_empty() {
            return Err(ApplicationError::invalid_input(
                "a user set must contain at least one account",
            ));
        }
        let unique_count = draft.members.iter().collect::<HashSet<_>>().len();
        if unique_count != draft.members.len() {
            return Err(ApplicationError::invalid_input(
                "a user set cannot contain duplicate accounts",
            ));
        }
        self.repository.save(draft).await
    }

    pub async fn delete(&self, user_set_id: &UserSetId) -> Result<(), ApplicationError> {
        self.repository.delete(user_set_id).await
    }
}
