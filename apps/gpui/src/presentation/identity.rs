use std::collections::HashMap;

use jira_domain::{Issue, IssueCommentAuthor, User};

/// Display-only directory for stable Jira identities.
///
/// Account IDs remain the domain identity and are never suitable UI labels. The directory starts
/// with the authenticated/user-search catalog, then fills missing entries from display metadata
/// carried by issue and comment payloads. Catalog entries win when the same account is present in
/// more than one source; embedded metadata still makes otherwise unknown users useful.
#[derive(Clone, Debug, Default)]
pub struct IdentityDirectory {
    names: HashMap<jira_domain::AccountId, String>,
}

impl IdentityDirectory {
    pub fn from_users(users: &[User]) -> Self {
        let mut directory = Self::default();
        for user in users {
            directory.insert(user.account_id.clone(), &user.display_name);
        }
        directory
    }

    pub fn include_issue(&mut self, issue: &Issue) {
        if let (Some(account_id), Some(display_name)) = (
            issue.assignee.as_ref(),
            issue.assignee_display_name.as_deref(),
        ) {
            self.insert_if_missing(account_id.clone(), display_name);
        }
        if let (Some(account_id), Some(display_name)) = (
            issue.reporter.as_ref(),
            issue.reporter_display_name.as_deref(),
        ) {
            self.insert_if_missing(account_id.clone(), display_name);
        }
    }

    pub fn include_comment_author(&mut self, author: Option<&IssueCommentAuthor>) {
        let Some(author) = author else {
            return;
        };
        if let Some(display_name) = author.display_name.as_deref() {
            self.insert_if_missing(author.account_id.clone(), display_name);
        }
    }

    fn insert(&mut self, account_id: jira_domain::AccountId, display_name: &str) {
        let display_name = display_name.trim();
        if !display_name.is_empty()
            && display_name != account_id.as_str().trim()
            && display_name.len() <= 255
        {
            self.names
                .entry(account_id)
                .or_insert_with(|| display_name.to_owned());
        }
    }

    fn insert_if_missing(&mut self, account_id: jira_domain::AccountId, display_name: &str) {
        self.insert(account_id, display_name);
    }

    pub fn display(&self, account_id: Option<&jira_domain::AccountId>, unassigned: &str) -> String {
        self.display_with_unknown(account_id, unassigned, "Unknown user")
    }

    fn display_with_unknown(
        &self,
        account_id: Option<&jira_domain::AccountId>,
        unassigned: &str,
        unknown: &str,
    ) -> String {
        let Some(account_id) = account_id else {
            return unassigned.to_owned();
        };
        self.names
            .get(account_id)
            .cloned()
            .unwrap_or_else(|| unknown.to_owned())
    }

    pub(super) fn display_author(&self, author: Option<&IssueCommentAuthor>) -> String {
        author
            .map(|author| {
                self.display_with_unknown(
                    Some(&author.account_id),
                    "Unknown author",
                    "Unknown author",
                )
            })
            .unwrap_or_else(|| "Unknown author".to_owned())
    }
}
