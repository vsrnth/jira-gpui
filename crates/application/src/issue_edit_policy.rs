use std::collections::HashSet;
use std::sync::{Arc, RwLock};

use jira_domain::{IssueId, IssueKey, JiraSiteId, Timestamp, User};
use time::Duration;

use crate::IssueLocator;

/// Policy shared by the issue-edit cache reads and transition invalidation.
///
/// The invalidation guard is deliberately local to the application service:
/// durable invalidation is best effort after a successful Jira write, while
/// this guard prevents a failed local invalidation from serving stale choices.
#[derive(Clone, Default)]
pub(crate) struct IssueEditCachePolicy {
    invalidated_transitions: Arc<RwLock<HashSet<IssueEditCacheKey>>>,
}

impl IssueEditCachePolicy {
    pub(crate) fn cache_is_fresh(fetched_at: Timestamp, now: Timestamp, ttl: Duration) -> bool {
        now >= fetched_at
            && fetched_at
                .checked_add(ttl)
                .is_some_and(|expires_at| now < expires_at)
    }

    pub(crate) fn filter_assignable_users(
        users: Vec<User>,
        query: &str,
        limit: usize,
    ) -> Vec<User> {
        let query = query.trim().to_lowercase();
        users
            .into_iter()
            .filter(|user| {
                query.is_empty()
                    || user.display_name.to_lowercase().contains(&query)
                    || user.account_id.as_str().to_lowercase().contains(&query)
            })
            .take(limit)
            .collect()
    }

    pub(crate) fn transitions_are_fresh(
        &self,
        site_id: &JiraSiteId,
        locator: &IssueLocator,
        fetched_at: Timestamp,
        now: Timestamp,
        ttl: Duration,
    ) -> bool {
        let key = IssueEditCacheKey::new(site_id, locator);
        let transition_was_invalidated = self
            .invalidated_transitions
            .read()
            .map(|keys| keys.contains(&key))
            .unwrap_or(true);
        !transition_was_invalidated && Self::cache_is_fresh(fetched_at, now, ttl)
    }

    pub(crate) fn mark_transitions_invalidated(
        &self,
        site_id: &JiraSiteId,
        locator: &IssueLocator,
    ) {
        if let Ok(mut keys) = self.invalidated_transitions.write() {
            keys.insert(IssueEditCacheKey::new(site_id, locator));
        }
    }

    pub(crate) fn mark_transitions_refreshed(&self, site_id: &JiraSiteId, locator: &IssueLocator) {
        if let Ok(mut keys) = self.invalidated_transitions.write() {
            keys.remove(&IssueEditCacheKey::new(site_id, locator));
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct IssueEditCacheKey {
    site_id: JiraSiteId,
    locator: IssueEditLocator,
}

impl IssueEditCacheKey {
    fn new(site_id: &JiraSiteId, locator: &IssueLocator) -> Self {
        Self {
            site_id: site_id.clone(),
            locator: IssueEditLocator::from(locator),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum IssueEditLocator {
    Id(IssueId),
    Key(IssueKey),
}

impl From<&IssueLocator> for IssueEditLocator {
    fn from(locator: &IssueLocator) -> Self {
        match locator {
            IssueLocator::Id(issue_id) => Self::Id(issue_id.clone()),
            IssueLocator::Key(issue_key) => Self::Key(issue_key.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use jira_domain::{AccountId, IssueId, IssueKey, JiraSiteId, User};
    use time::macros::datetime;

    use super::*;

    fn site(value: &str) -> JiraSiteId {
        JiraSiteId::new(value).expect("site")
    }

    fn issue_id(value: &str) -> IssueLocator {
        IssueLocator::Id(IssueId::new(value).expect("issue id"))
    }

    fn issue_key(value: &str) -> IssueLocator {
        IssueLocator::Key(IssueKey::new(value).expect("issue key"))
    }

    #[test]
    fn freshness_excludes_future_and_exactly_expired_entries() {
        let fetched_at = datetime!(2026-01-01 00:00 UTC);
        let ttl = Duration::hours(24);
        assert!(IssueEditCachePolicy::cache_is_fresh(
            fetched_at,
            datetime!(2026-01-01 23:59:59.999999999 UTC),
            ttl,
        ));
        assert!(!IssueEditCachePolicy::cache_is_fresh(
            fetched_at,
            datetime!(2026-01-02 00:00 UTC),
            ttl,
        ));
        assert!(!IssueEditCachePolicy::cache_is_fresh(
            datetime!(2026-01-02 00:00 UTC),
            datetime!(2026-01-01 00:00 UTC),
            ttl,
        ));
    }

    #[test]
    fn cached_user_filter_matches_display_name_and_account_case_insensitively() {
        let users = vec![
            User::new(
                site("site-a"),
                AccountId::new("alice-id").expect("account"),
                "Alice Example",
                None,
                true,
            ),
            User::new(
                site("site-a"),
                AccountId::new("bob-id").expect("account"),
                "Bob Example",
                None,
                true,
            ),
        ];

        let by_name = IssueEditCachePolicy::filter_assignable_users(users.clone(), "ALICE", 10);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].display_name, "Alice Example");

        let by_account = IssueEditCachePolicy::filter_assignable_users(users.clone(), "BOB-ID", 10);
        assert_eq!(by_account.len(), 1);
        assert_eq!(by_account[0].account_id.as_str(), "bob-id");

        let bounded = IssueEditCachePolicy::filter_assignable_users(users, "", 1);
        assert_eq!(bounded.len(), 1);
        assert_eq!(bounded[0].display_name, "Alice Example");
    }

    #[test]
    fn transition_invalidation_isolated_by_site_and_locator_kind() {
        let policy = IssueEditCachePolicy::default();
        let site_a = site("site-a");
        let site_b = site("site-b");
        let id = issue_id("100");
        let key = issue_key("APP-100");
        let fetched_at = datetime!(2026-01-01 00:00 UTC);
        let now = datetime!(2026-01-01 00:01 UTC);
        let ttl = Duration::hours(24);

        policy.mark_transitions_invalidated(&site_a, &id);

        assert!(!policy.transitions_are_fresh(&site_a, &id, fetched_at, now, ttl));
        assert!(policy.transitions_are_fresh(&site_a, &key, fetched_at, now, ttl));
        assert!(policy.transitions_are_fresh(&site_b, &id, fetched_at, now, ttl));

        policy.mark_transitions_refreshed(&site_a, &id);
        assert!(policy.transitions_are_fresh(&site_a, &id, fetched_at, now, ttl));
    }
}
