use crate::event_semantics::{
    normalize_matching_user_set_ids, same_event_identity, union_matching_user_set_ids,
};

use std::{
    cmp::Reverse,
    collections::{HashMap, HashSet},
    mem::discriminant,
    sync::{Arc, RwLock},
};

use jira_application::{
    ApplicationError, AttachmentImage, CachedAssignableUsers, CachedIssueTransitions,
    CommitOutcome, ErrorKind, IssueCachePort, IssueEditCachePort, IssueListQuery, IssueLocator,
    IssueMediaCachePort, IssueTransition, MAX_ASSIGNABLE_USER_SEARCH_LIMIT,
    MAX_CACHED_ATTACHMENT_IMAGE_BYTES, MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES,
    MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES, MAX_ISSUE_TRANSITIONS, PortFuture, SyncCommit,
    SyncState, UpdateFeedPort, UpdateFeedQuery, UserSetDraft, UserSetPort, validate_cached_image,
};
use jira_domain::{
    EventId, Issue, IssueId, JiraSiteId, NotificationDelivery, Timestamp, UpdateEvent,
    UpdateReadState, User, UserSet, UserSetId,
};
use time::OffsetDateTime;

type IssueKey = (JiraSiteId, IssueId);
type UserSetKey = (JiraSiteId, UserSetId);
type EditCacheKey = (JiraSiteId, String, String);
type MediaCacheKey = (JiraSiteId, IssueId, String);

struct StoredMediaImage {
    image: AttachmentImage,
    order: u64,
}

#[derive(Default)]
struct State {
    issues: HashMap<IssueKey, Issue>,
    membership: HashMap<UserSetKey, HashSet<IssueId>>,
    sync_states: HashMap<UserSetKey, SyncState>,
    events: Vec<UpdateEvent>,
    user_sets: HashMap<UserSetKey, UserSet>,
    assignable_users: HashMap<EditCacheKey, CachedAssignableUsers>,
    issue_transitions: HashMap<EditCacheKey, CachedIssueTransitions>,
    media_images: HashMap<MediaCacheKey, StoredMediaImage>,
    next_media_order: u64,
    next_user_set_id: u64,
}

fn edit_cache_key(site_id: &JiraSiteId, locator: &IssueLocator) -> EditCacheKey {
    match locator {
        IssueLocator::Id(id) => (site_id.clone(), "id".to_owned(), id.as_str().to_owned()),
        IssueLocator::Key(key) => (site_id.clone(), "key".to_owned(), key.as_str().to_owned()),
    }
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
    fn cache_detail_issue<'a>(&'a self, issue: &'a Issue) -> PortFuture<'a, bool> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            let Some(existing) = state
                .issues
                .get_mut(&(issue.site_id.clone(), issue.id.clone()))
            else {
                return Ok(false);
            };
            if existing == issue {
                return Ok(false);
            }
            *existing = issue.clone();
            Ok(true)
        })
    }

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
                let key = (commit.site_id.clone(), issue.id.clone());
                let issue = if !issue.detail_loaded {
                    state
                        .issues
                        .get(&key)
                        .map(|existing| {
                            let mut merged = issue.clone();
                            merged.description_text = existing.description_text.clone();
                            merged.rich_description = existing.rich_description.clone();
                            merged.detail_loaded = existing.detail_loaded;
                            merged
                        })
                        .unwrap_or(issue)
                } else {
                    issue
                };
                state.issues.insert(key, issue);
            }

            let mut inserted_events: Vec<UpdateEvent> = Vec::new();
            for event in commit.update_events {
                if let Some(existing) = state
                    .events
                    .iter_mut()
                    .find(|current| current.id == event.id)
                {
                    union_matching_user_set_ids(existing, &event);
                } else if let Some(existing) = inserted_events
                    .iter_mut()
                    .find(|current| current.id == event.id)
                {
                    union_matching_user_set_ids(existing, &event);
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
            sync_state.last_error_kind = Some(if kind == ErrorKind::UnknownOutcome {
                ErrorKind::Upstream
            } else {
                kind
            });
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
            events.sort_by_key(|event| Reverse(event.occurred_at));
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

impl IssueEditCachePort for InMemoryStore {
    fn cached_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, Option<CachedAssignableUsers>> {
        Box::pin(async move {
            Ok(self
                .read_state()?
                .assignable_users
                .get(&edit_cache_key(site_id, locator))
                .cloned())
        })
    }

    fn replace_assignable_users<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
        users: Vec<User>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            if users.len() > MAX_ASSIGNABLE_USER_SEARCH_LIMIT
                || users.iter().any(|user| user.site_id != *site_id)
            {
                return Err(ApplicationError::invalid_input(
                    "cached assignable user belongs to another site",
                ));
            }
            self.write_state()?.assignable_users.insert(
                edit_cache_key(site_id, locator),
                CachedAssignableUsers { users, fetched_at },
            );
            Ok(())
        })
    }

    fn cached_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, Option<CachedIssueTransitions>> {
        Box::pin(async move {
            Ok(self
                .read_state()?
                .issue_transitions
                .get(&edit_cache_key(site_id, locator))
                .cloned())
        })
    }

    fn replace_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
        transitions: Vec<IssueTransition>,
        fetched_at: Timestamp,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            if transitions.len() > MAX_ISSUE_TRANSITIONS {
                return Err(ApplicationError::invalid_input(
                    "cached transition list exceeds configured limit",
                ));
            }
            self.write_state()?.issue_transitions.insert(
                edit_cache_key(site_id, locator),
                CachedIssueTransitions {
                    transitions,
                    fetched_at,
                },
            );
            Ok(())
        })
    }

    fn invalidate_issue_transitions<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        locator: &'a IssueLocator,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.write_state()?
                .issue_transitions
                .remove(&edit_cache_key(site_id, locator));
            Ok(())
        })
    }
}

impl IssueMediaCachePort for InMemoryStore {
    fn cached_attachment_image<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
        attachment_id: &'a str,
    ) -> PortFuture<'a, Option<AttachmentImage>> {
        Box::pin(async move {
            let mut state = self.write_state()?;
            let key = (site_id.clone(), issue_id.clone(), attachment_id.to_owned());
            let Some(stored) = state.media_images.get(&key) else {
                return Ok(None);
            };
            if validate_cached_image(
                &stored.image,
                attachment_id,
                MAX_CACHED_ATTACHMENT_IMAGE_BYTES,
            )
            .is_err()
            {
                state.media_images.remove(&key);
                return Ok(None);
            }
            Ok(Some(stored.image.clone()))
        })
    }

    fn cache_attachment_image<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
        image: &'a AttachmentImage,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            validate_cached_image(
                image,
                &image.attachment_id,
                MAX_CACHED_ATTACHMENT_IMAGE_BYTES,
            )?;
            let mut state = self.write_state()?;
            let key = (
                site_id.clone(),
                issue_id.clone(),
                image.attachment_id.clone(),
            );
            state.media_images.remove(&key);
            state.next_media_order = state.next_media_order.saturating_add(1);
            while state.media_images.len() + 1 > MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES
                || state
                    .media_images
                    .values()
                    .map(|stored| stored.image.bytes.len())
                    .sum::<usize>()
                    .saturating_add(image.bytes.len())
                    > MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES
            {
                let Some(oldest) = state
                    .media_images
                    .iter()
                    .min_by(|(left_key, left), (right_key, right)| {
                        left.order
                            .cmp(&right.order)
                            .then_with(|| left_key.cmp(right_key))
                    })
                    .map(|(key, _)| key.clone())
                else {
                    break;
                };
                state.media_images.remove(&oldest);
            }
            if image.bytes.len() <= MAX_CACHED_ATTACHMENT_IMAGE_TOTAL_BYTES {
                let order = state.next_media_order;
                state.media_images.insert(
                    key,
                    StoredMediaImage {
                        image: image.clone(),
                        order,
                    },
                );
            }
            Ok(())
        })
    }

    fn remove_cached_attachment_image<'a>(
        &'a self,
        site_id: &'a JiraSiteId,
        issue_id: &'a IssueId,
        attachment_id: &'a str,
    ) -> PortFuture<'a, ()> {
        Box::pin(async move {
            self.write_state()?.media_images.remove(&(
                site_id.clone(),
                issue_id.clone(),
                attachment_id.to_owned(),
            ));
            Ok(())
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

#[cfg(test)]
mod tests {
    use futures_lite::future::block_on;
    use jira_application::{
        AttachmentImage, IssueCachePort, IssueMediaCachePort, SyncCommit, SyncState,
    };
    use jira_domain::{IssueId, JiraSiteId, UserSetId};

    use super::InMemoryStore;

    fn site_id() -> JiraSiteId {
        JiraSiteId::new("cloud-id").expect("valid site id")
    }

    #[test]
    fn media_cache_isolated_and_invalid_entries_are_rejected() {
        let store = InMemoryStore::new();
        let site = site_id();
        let issue = IssueId::new("issue").expect("issue");
        let image = AttachmentImage {
            attachment_id: "image".into(),
            mime_type: "image/png".into(),
            bytes: b"\x89PNG\r\n\x1a\nvalid".to_vec(),
        };
        block_on(store.cache_attachment_image(&site, &issue, &image)).expect("cache image");
        assert!(
            block_on(store.cached_attachment_image(&site, &issue, "image"))
                .expect("read image")
                .is_some()
        );
        assert!(
            block_on(store.cached_attachment_image(
                &JiraSiteId::new("other-site").expect("site"),
                &issue,
                "image"
            ))
            .expect("isolated read")
            .is_none()
        );
        let invalid = AttachmentImage {
            attachment_id: "invalid".into(),
            mime_type: "image/png".into(),
            bytes: b"not-an-image".to_vec(),
        };
        assert!(block_on(store.cache_attachment_image(&site, &issue, &invalid)).is_err());
    }

    #[test]
    fn media_cache_evicts_oldest_entry_at_deterministic_entry_bound() {
        let store = InMemoryStore::new();
        let site = site_id();
        let issue = IssueId::new("issue").expect("issue");
        for index in 0..=jira_application::MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES {
            let image = AttachmentImage {
                attachment_id: format!("image-{index}"),
                mime_type: "image/png".into(),
                bytes: b"\x89PNG\r\n\x1a\nvalid".to_vec(),
            };
            block_on(store.cache_attachment_image(&site, &issue, &image)).expect("cache image");
        }
        assert!(
            block_on(store.cached_attachment_image(&site, &issue, "image-0"))
                .expect("oldest read")
                .is_none()
        );
        assert!(
            block_on(store.cached_attachment_image(
                &site,
                &issue,
                &format!(
                    "image-{}",
                    jira_application::MAX_CACHED_ATTACHMENT_IMAGE_ENTRIES
                )
            ))
            .expect("newest read")
            .is_some()
        );
    }

    #[test]
    fn media_cache_evicts_oldest_entry_at_deterministic_aggregate_byte_bound() {
        let store = InMemoryStore::new();
        let site = site_id();
        let issue = IssueId::new("issue").expect("issue");
        let mut bytes = vec![0; jira_application::MAX_CACHED_ATTACHMENT_IMAGE_BYTES];
        bytes[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        for index in 0..5 {
            let image = AttachmentImage {
                attachment_id: format!("large-image-{index}"),
                mime_type: "image/png".into(),
                bytes: bytes.clone(),
            };
            block_on(store.cache_attachment_image(&site, &issue, &image)).expect("cache image");
        }
        assert!(
            block_on(store.cached_attachment_image(&site, &issue, "large-image-0"))
                .expect("oldest read")
                .is_none()
        );
        assert!(
            block_on(store.cached_attachment_image(&site, &issue, "large-image-4"))
                .expect("newest read")
                .is_some()
        );
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
}
