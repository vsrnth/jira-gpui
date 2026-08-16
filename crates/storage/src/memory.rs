use std::{
    collections::{HashMap, HashSet},
    mem::discriminant,
    sync::{Arc, RwLock},
};

use jira_application::{
    ApplicationError, CommitOutcome, ErrorKind, IssueCachePort, IssueListQuery, PortFuture,
    SyncCommit, SyncState, UpdateFeedPort, UpdateFeedQuery, UserSetDraft, UserSetPort,
};
use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, UpdateEvent, UpdateReadState,
    UserSet, UserSetId,
};
use time::OffsetDateTime;

type IssueKey = (JiraSiteId, IssueId);
type UserSetKey = (JiraSiteId, UserSetId);

#[derive(Default)]
struct State {
    issues: HashMap<IssueKey, Issue>,
    membership: HashMap<UserSetKey, HashSet<IssueId>>,
    sync_states: HashMap<UserSetKey, SyncState>,
    events: Vec<UpdateEvent>,
    user_sets: HashMap<UserSetKey, UserSet>,
    next_user_set_id: u64,
}

/// Thread-safe development adapter for the application persistence ports.
///
/// It gives the GPUI shell and application services a real adapter contract
/// before SQLite migrations are introduced. No presentation code depends on
/// this concrete type, so replacing it does not affect GPUI or a future Tauri
/// frontend.
#[derive(Clone, Default)]
pub struct InMemoryStore {
    state: Arc<RwLock<State>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn read_state(&self) -> Result<std::sync::RwLockReadGuard<'_, State>, ApplicationError> {
        self.state
            .read()
            .map_err(|_| storage_error("in-memory store read lock was poisoned"))
    }

    fn write_state(&self) -> Result<std::sync::RwLockWriteGuard<'_, State>, ApplicationError> {
        self.state
            .write()
            .map_err(|_| storage_error("in-memory store write lock was poisoned"))
    }
}

impl IssueCachePort for InMemoryStore {
    fn list_issues<'a>(&'a self, query: &'a IssueListQuery) -> PortFuture<'a, Vec<Issue>> {
        Box::pin(async move {
            let state = self.read_state()?;
            let member_ids = state
                .membership
                .get(&(query.site_id.clone(), query.user_set_id.clone()));
            let normalized_text = query
                .text
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_lowercase);

            let mut issues = state
                .issues
                .values()
                .filter(|issue| issue.site_id == query.site_id)
                .filter(|issue| member_ids.is_some_and(|ids| ids.contains(&issue.id)))
                .filter(|issue| {
                    query.assignees.is_empty()
                        || issue
                            .assignee
                            .as_ref()
                            .is_some_and(|assignee| query.assignees.contains(assignee))
                })
                .filter(|issue| {
                    normalized_text.as_ref().is_none_or(|text| {
                        issue.key.as_str().to_lowercase().contains(text)
                            || issue.summary.to_lowercase().contains(text)
                    })
                })
                .cloned()
                .collect::<Vec<_>>();

            issues.sort_by(|left, right| {
                right
                    .updated_at
                    .cmp(&left.updated_at)
                    .then_with(|| left.key.cmp(&right.key))
            });

            Ok(issues
                .into_iter()
                .skip(query.offset)
                .take(query.limit)
                .collect())
        })
    }

    fn get_issue<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
    ) -> PortFuture<'a, Option<Issue>> {
        Box::pin(async move {
            Ok(self
                .read_state()?
                .issues
                .get(&(site_id.clone(), issue_id.clone()))
                .cloned())
        })
    }

    fn issues_for_user_set<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Vec<Issue>> {
        Box::pin(async move {
            let state = self.read_state()?;
            let ids = state
                .membership
                .get(&(site_id.clone(), user_set_id.clone()));
            Ok(state
                .issues
                .values()
                .filter(|issue| issue.site_id == *site_id)
                .filter(|issue| ids.is_some_and(|ids| ids.contains(&issue.id)))
                .cloned()
                .collect())
        })
    }

    fn sync_state<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
    ) -> PortFuture<'a, Option<SyncState>> {
        Box::pin(async move {
            Ok(self
                .read_state()?
                .sync_states
                .get(&(site_id.clone(), user_set_id.clone()))
                .cloned())
        })
    }

    fn commit_sync<'a>(&'a self, commit: SyncCommit) -> PortFuture<'a, CommitOutcome> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            if commit.state.site_id != commit.site_id
                || commit.state.user_set_id != commit.user_set_id
            {
                return Err(ApplicationError::invalid_input(
                    "sync state does not match commit",
                ));
            }
            if commit
                .issues
                .iter()
                .any(|issue| issue.site_id != commit.site_id)
            {
                return Err(ApplicationError::invalid_input(
                    "issue site does not match commit",
                ));
            }
            let mut incoming_events = HashMap::new();
            for event in &commit.update_events {
                if event.site_id != commit.site_id {
                    return Err(ApplicationError::invalid_input(
                        "event site does not match commit",
                    ));
                }
                if let Some(existing) = state.events.iter().find(|current| current.id == event.id)
                    && !same_event_identity(existing, event)
                {
                    return Err(ApplicationError::invalid_input(
                        "event ID conflicts with a different update event",
                    ));
                }
                if let Some(existing) = incoming_events.get(&event.id)
                    && !same_event_identity(existing, event)
                {
                    return Err(ApplicationError::invalid_input(
                        "event ID conflicts with a different update event",
                    ));
                }
                incoming_events.insert(event.id.clone(), event.clone());
            }
            let membership_key = (commit.site_id.clone(), commit.user_set_id.clone());
            let incoming_ids = commit
                .issues
                .iter()
                .map(|issue| issue.id.clone())
                .collect::<HashSet<_>>();

            if commit.replace_membership {
                state.membership.insert(membership_key, incoming_ids);
            } else {
                state
                    .membership
                    .entry(membership_key)
                    .or_default()
                    .extend(incoming_ids);
            }

            for issue in commit.issues {
                state
                    .issues
                    .insert((commit.site_id.clone(), issue.id.clone()), issue);
            }

            let mut inserted_events: Vec<UpdateEvent> = Vec::new();
            for event in commit.update_events {
                if let Some(existing) = state
                    .events
                    .iter_mut()
                    .find(|current| current.id == event.id)
                {
                    for user_set_id in event.matching_user_set_ids {
                        if !existing.matching_user_set_ids.contains(&user_set_id) {
                            existing.matching_user_set_ids.push(user_set_id);
                        }
                    }
                } else if let Some(existing) = inserted_events
                    .iter_mut()
                    .find(|current| current.id == event.id)
                {
                    for user_set_id in event.matching_user_set_ids {
                        if !existing.matching_user_set_ids.contains(&user_set_id) {
                            existing.matching_user_set_ids.push(user_set_id);
                        }
                    }
                } else {
                    inserted_events.push(event);
                }
            }
            for event in &mut inserted_events {
                normalize_matching_user_set_ids(event);
            }
            for event in &mut state.events {
                normalize_matching_user_set_ids(event);
            }
            state.events.extend(inserted_events.iter().cloned());
            state
                .sync_states
                .insert((commit.site_id, commit.user_set_id), commit.state);

            Ok(CommitOutcome { inserted_events })
        })
    }

    fn record_sync_failure<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        user_set_id: &'a UserSetId,
        kind: ErrorKind,
        _at: jira_domain::Timestamp,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            let sync_state = state
                .sync_states
                .entry((site_id.clone(), user_set_id.clone()))
                .or_insert_with(|| SyncState::new(site_id.clone(), user_set_id.clone()));
            sync_state.consecutive_failures = sync_state.consecutive_failures.saturating_add(1);
            sync_state.last_error_kind = Some(kind);
            Ok(())
        })
    }

    fn record_notification_delivery<'a>(
        &'a self,
        event_id: &'a EventId,
        delivery: NotificationDelivery,
        _at: jira_domain::Timestamp,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            if let Some(event) = self
                .write_state()?
                .events
                .iter_mut()
                .find(|event| event.id == *event_id)
            {
                event.record_notification_delivery(delivery);
            }
            Ok(())
        })
    }
}

impl UpdateFeedPort for InMemoryStore {
    fn list<'a>(&'a self, query: &'a UpdateFeedQuery) -> PortFuture<'a, Vec<UpdateEvent>> {
        Box::pin(async move {
            let mut events = self
                .read_state()?
                .events
                .iter()
                .filter(|event| event.site_id == query.site_id)
                .filter(|event| !query.unread_only || event.read_state == UpdateReadState::Unread)
                .filter(|event| query.before.is_none_or(|before| event.occurred_at < before))
                .filter(|event| {
                    query.kinds.is_empty()
                        || query
                            .kinds
                            .iter()
                            .any(|kind| discriminant(kind) == discriminant(&event.kind))
                })
                .cloned()
                .collect::<Vec<_>>();
            events.sort_by(|left, right| right.occurred_at.cmp(&left.occurred_at));
            events.truncate(query.limit);
            Ok(events)
        })
    }

    fn unread_count<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        Box::pin(async move {
            Ok(self
                .read_state()?
                .events
                .iter()
                .filter(|event| {
                    event.site_id == *site_id && event.read_state == UpdateReadState::Unread
                })
                .count())
        })
    }

    fn mark_read<'a>(&'a self, event_ids: &'a [EventId], read: bool) -> PortFuture<'a, usize> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            let mut changed = 0;
            for event in &mut state.events {
                if event_ids.contains(&event.id) {
                    let desired = if read {
                        UpdateReadState::Read
                    } else {
                        UpdateReadState::Unread
                    };
                    if event.read_state != desired {
                        event.read_state = desired;
                        changed += 1;
                    }
                }
            }
            Ok(changed)
        })
    }

    fn mark_all_read<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, usize> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            let mut changed = 0;
            for event in &mut state.events {
                if event.site_id == *site_id && event.read_state == UpdateReadState::Unread {
                    event.mark_read();
                    changed += 1;
                }
            }
            Ok(changed)
        })
    }
}

impl UserSetPort for InMemoryStore {
    fn list<'a>(&'a self, site_id: &'a JiraSiteId) -> PortFuture<'a, Vec<UserSet>> {
        Box::pin(async move {
            let mut sets = self
                .read_state()?
                .user_sets
                .values()
                .filter(|set| set.site_id == *site_id)
                .cloned()
                .collect::<Vec<_>>();
            sets.sort_by(|left, right| left.name.cmp(&right.name));
            Ok(sets)
        })
    }

    fn save<'a>(&'a self, draft: UserSetDraft) -> PortFuture<'a, UserSet> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            state.next_user_set_id = state.next_user_set_id.saturating_add(1);
            let id = UserSetId::new(format!("user-set-{}", state.next_user_set_id))
                .map_err(|error| storage_error(error.to_string()))?;
            let set = UserSet::new(
                id.clone(),
                draft.site_id,
                draft.name,
                draft.members,
                OffsetDateTime::now_utc(),
            )
            .map_err(|error| ApplicationError::invalid_input(error.to_string()))?;
            state
                .user_sets
                .insert((set.site_id.clone(), id), set.clone());
            Ok(set)
        })
    }

    fn delete<'a>(&'a self, user_set_id: &'a UserSetId) -> PortFuture<'a, ()> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            state.user_sets.retain(|(_, id), _| id != user_set_id);
            state
                .membership
                .retain(|(_, set_id), _| set_id != user_set_id);
            state
                .sync_states
                .retain(|(_, set_id), _| set_id != user_set_id);
            for event in &mut state.events {
                event
                    .matching_user_set_ids
                    .retain(|set_id| set_id != user_set_id);
                normalize_matching_user_set_ids(event);
            }
            Ok(())
        })
    }
}

fn storage_error(message: impl Into<String>) -> ApplicationError {
    ApplicationError::new(ErrorKind::Storage, message)
}

fn same_event_identity(left: &UpdateEvent, right: &UpdateEvent) -> bool {
    left.id == right.id
        && left.site_id == right.site_id
        && left.issue_id == right.issue_id
        && left.issue_key == right.issue_key
        && left.kind == right.kind
        && left.occurred_at == right.occurred_at
}

fn normalize_matching_user_set_ids(event: &mut UpdateEvent) {
    event.matching_user_set_ids.sort();
    event.matching_user_set_ids.dedup();
}

#[cfg(test)]
mod tests {
    use futures_lite::future::block_on;
    use jira_application::{
        ErrorKind, IssueCachePort, SyncCommit, SyncState, UpdateFeedPort, UpdateFeedQuery,
        UserSetDraft, UserSetPort,
    };
    use jira_domain::{
        AccountId, EventId, Issue, IssueId, IssueKey, IssueType, JiraSiteId, Priority, Project,
        Status, UpdateEvent, UpdateKind, UserSetId,
    };
    use time::macros::datetime;

    use super::InMemoryStore;

    fn site_id() -> JiraSiteId {
        JiraSiteId::new("cloud-id").expect("valid site id")
    }

    fn issue(site_id: JiraSiteId) -> Issue {
        Issue::new(
            site_id,
            IssueId::new("100").expect("valid issue id"),
            IssueKey::new("APP-100").expect("valid issue key"),
            Project {
                id: "10".into(),
                key: "APP".into(),
                name: "Application".into(),
            },
            IssueType {
                id: "1".into(),
                name: "Task".into(),
                icon_url: None,
            },
            "Issue",
            Status {
                id: "open".into(),
                name: "Open".into(),
                category: None,
            },
            Priority {
                id: None,
                name: None,
                icon_url: None,
            },
            None,
            None,
            None,
            vec![],
            datetime!(2026-01-01 00:00 UTC),
            datetime!(2026-01-02 00:00 UTC),
            None,
        )
    }

    #[test]
    fn user_sets_are_saved_without_exposing_storage_to_the_ui() {
        let store = InMemoryStore::new();
        let saved = block_on(store.save(UserSetDraft {
            site_id: site_id(),
            name: "Backend team".into(),
            members: vec![AccountId::new("account-1").expect("valid account id")],
        }))
        .expect("save succeeds");

        let sets = block_on(UserSetPort::list(&store, &site_id())).expect("list succeeds");
        assert_eq!(sets, vec![saved]);
    }

    #[test]
    fn commit_persists_sync_state_atomically_with_an_empty_page() {
        let store = InMemoryStore::new();
        let site = site_id();
        let user_set = UserSetId::new("team").expect("valid user set id");
        let sync_state = SyncState::new(site.clone(), user_set.clone());

        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: user_set.clone(),
            issues: Vec::new(),
            update_events: Vec::new(),
            replace_membership: true,
            state: sync_state.clone(),
        }))
        .expect("commit succeeds");

        let persisted = block_on(store.sync_state(&site, &user_set))
            .expect("state lookup succeeds")
            .expect("sync state exists");
        assert_eq!(persisted.site_id, sync_state.site_id);
        assert_eq!(persisted.user_set_id, sync_state.user_set_id);
        assert_eq!(persisted.consecutive_failures, 0);
    }

    #[test]
    fn missing_membership_does_not_expose_site_issues() {
        let store = InMemoryStore::new();
        let site = site_id();
        let user_set = UserSetId::new("team").expect("valid user set");
        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: user_set,
            issues: vec![issue(site.clone())],
            update_events: vec![],
            replace_membership: true,
            state: SyncState::new(
                site.clone(),
                UserSetId::new("team").expect("valid user set"),
            ),
        }))
        .expect("commit succeeds");
        let missing = UserSetId::new("never-synced").expect("valid user set");
        let query = jira_application::IssueListQuery {
            site_id: site,
            user_set_id: missing,
            text: None,
            assignees: vec![],
            limit: 10,
            offset: 0,
        };
        assert!(
            block_on(store.list_issues(&query))
                .expect("list succeeds")
                .is_empty()
        );
    }

    #[test]
    fn user_sets_are_site_isolated_and_members_preserve_order() {
        let store = InMemoryStore::new();
        let first_site = JiraSiteId::new("site-a").expect("site");
        let second_site = JiraSiteId::new("site-b").expect("site");
        let first = block_on(store.save(UserSetDraft {
            site_id: first_site.clone(),
            name: "Team".into(),
            members: vec![
                AccountId::new("second").expect("account"),
                AccountId::new("first").expect("account"),
            ],
        }))
        .expect("save");
        let second = block_on(store.save(UserSetDraft {
            site_id: second_site.clone(),
            name: "Team".into(),
            members: vec![AccountId::new("other").expect("account")],
        }))
        .expect("save");
        assert_eq!(
            block_on(UserSetPort::list(&store, &first_site)).expect("list"),
            vec![first.clone()]
        );
        assert_eq!(
            block_on(UserSetPort::list(&store, &second_site)).expect("list"),
            vec![second]
        );
        assert_eq!(first.members[0].as_str(), "second");
        assert_eq!(first.members[1].as_str(), "first");
    }

    #[test]
    fn duplicate_events_union_associations_and_reject_conflicts() {
        let store = InMemoryStore::new();
        let site = site_id();
        let first_set = UserSetId::new("first").expect("set");
        let second_set = UserSetId::new("second").expect("set");
        let issue = issue(site.clone());
        let event = UpdateEvent::new(
            EventId::new("event").expect("event"),
            site.clone(),
            issue.id.clone(),
            issue.key.clone(),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![first_set.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: first_set.clone(),
            issues: vec![issue.clone()],
            update_events: vec![event.clone()],
            replace_membership: true,
            state: SyncState::new(site.clone(), first_set),
        }))
        .expect("first commit");
        let mut union_event = event.clone();
        union_event.matching_user_set_ids = vec![second_set.clone()];
        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: second_set.clone(),
            issues: vec![],
            update_events: vec![union_event],
            replace_membership: false,
            state: SyncState::new(site.clone(), second_set),
        }))
        .expect("union commit");
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id: site.clone(),
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("list events");
        assert_eq!(events[0].matching_user_set_ids.len(), 2);
        let mut conflict = event;
        conflict.kind = UpdateKind::CommentAdded {
            comment_id: "comment".into(),
            author: None,
            excerpt: "changed".into(),
        };
        assert!(
            block_on(store.commit_sync(SyncCommit {
                site_id: site.clone(),
                user_set_id: UserSetId::new("third").expect("set"),
                issues: vec![],
                update_events: vec![conflict],
                replace_membership: false,
                state: SyncState::new(site, UserSetId::new("third").expect("set")),
            }))
            .is_err()
        );
    }

    #[test]
    fn record_sync_failure_upserts_and_increments_state() {
        let store = InMemoryStore::new();
        let site = site_id();
        let user_set = UserSetId::new("team").expect("set");
        block_on(store.record_sync_failure(
            &site,
            &user_set,
            ErrorKind::Offline,
            datetime!(2026-01-03 00:00 UTC),
        ))
        .expect("failure");
        block_on(store.record_sync_failure(
            &site,
            &user_set,
            ErrorKind::Upstream,
            datetime!(2026-01-03 00:01 UTC),
        ))
        .expect("failure");
        let state = block_on(store.sync_state(&site, &user_set))
            .expect("state")
            .expect("state exists");
        assert_eq!(state.consecutive_failures, 2);
        assert_eq!(state.last_error_kind, Some(ErrorKind::Upstream));
    }

    #[test]
    fn invalid_commit_is_atomic_and_leaves_existing_state_unchanged() {
        let store = InMemoryStore::new();
        let site = site_id();
        let user_set = UserSetId::new("team").expect("set");
        let existing_issue = issue(site.clone());
        let existing_state = SyncState::new(site.clone(), user_set.clone());
        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: user_set.clone(),
            issues: vec![existing_issue.clone()],
            update_events: vec![],
            replace_membership: true,
            state: existing_state.clone(),
        }))
        .expect("initial commit");
        let other_site = JiraSiteId::new("other-site").expect("site");
        assert!(
            block_on(store.commit_sync(SyncCommit {
                site_id: site.clone(),
                user_set_id: user_set.clone(),
                issues: vec![issue(other_site)],
                update_events: vec![],
                replace_membership: true,
                state: SyncState::new(site.clone(), UserSetId::new("wrong").expect("set")),
            }))
            .is_err()
        );
        assert_eq!(
            block_on(store.get_issue(&site, &existing_issue.id)).expect("issue"),
            Some(existing_issue.clone())
        );
        let persisted_state = block_on(store.sync_state(&site, &user_set))
            .expect("state")
            .expect("state exists");
        assert_eq!(persisted_state.site_id, existing_state.site_id);
        assert_eq!(persisted_state.user_set_id, existing_state.user_set_id);
        assert_eq!(
            persisted_state.consecutive_failures,
            existing_state.consecutive_failures
        );
        assert!(
            block_on(store.commit_sync(SyncCommit {
                site_id: site.clone(),
                user_set_id: user_set.clone(),
                issues: vec![issue(JiraSiteId::new("other-site").expect("site"))],
                update_events: vec![],
                replace_membership: true,
                state: SyncState::new(site.clone(), user_set.clone()),
            }))
            .is_err()
        );
        assert_eq!(
            block_on(store.get_issue(&site, &existing_issue.id)).expect("issue"),
            Some(existing_issue)
        );
    }

    #[test]
    fn deleting_user_set_cascades_local_state_and_event_associations() {
        let store = InMemoryStore::new();
        let site = site_id();
        let user_set = block_on(store.save(UserSetDraft {
            site_id: site.clone(),
            name: "Team".into(),
            members: vec![],
        }))
        .expect("set")
        .id;
        let event = UpdateEvent::new(
            EventId::new("event").expect("event"),
            site.clone(),
            IssueId::new("100").expect("issue"),
            IssueKey::new("APP-100").expect("key"),
            UpdateKind::IssueAddedToView,
            datetime!(2026-01-03 00:00 UTC),
            vec![user_set.clone()],
        );
        block_on(store.commit_sync(SyncCommit {
            site_id: site.clone(),
            user_set_id: user_set.clone(),
            issues: vec![],
            update_events: vec![event],
            replace_membership: true,
            state: SyncState::new(site.clone(), user_set.clone()),
        }))
        .expect("commit");
        block_on(UserSetPort::delete(&store, &user_set)).expect("delete");
        assert!(
            block_on(store.sync_state(&site, &user_set))
                .expect("state")
                .is_none()
        );
        let events = block_on(UpdateFeedPort::list(
            &store,
            &UpdateFeedQuery {
                site_id: site,
                unread_only: false,
                kinds: vec![],
                before: None,
                limit: 10,
            },
        ))
        .expect("events");
        assert!(events[0].matching_user_set_ids.is_empty());
    }
}
